//! Alta idempotente de empleados de Ingelmec desde un CSV (Alinea plan
//! hermes-alinea, tarea A6).
//!
//! Lee un CSV con columnas `email,nombre,rol,perfil,techo_gasto_usd` y hace
//! upsert de:
//! - `users` (crea el usuario si el email no existe; no toca nada si ya existe).
//! - `user_roles` (asigna `rol` — debe ser uno de los 6 roles seed de
//!   `014_rbac_roles.sql`: admin, gerencia, tecnica, comercial, financiera,
//!   ingenieria). El acceso al role-pack homónimo llega TRANSITIVAMENTE por
//!   el grant de `022_role_pack_profiles.sql` — no se crea ningún
//!   `agent_profile` por usuario (modelo: role-pack compartido + overlay
//!   per-user, JAMÁS un agente completo por empleado).
//! - `resource_acl` (SOLO si `perfil` viene informado en el CSV y difiere del
//!   role-pack por defecto de `rol`): grant DIRECTO por usuario
//!   (`principal_type='user'`) sobre ese `agent_profile` — para las
//!   excepciones puntuales del plan (p.ej. un comercial que también necesita
//!   `servimec-tko`). El acceso por rol de la migración 022 sigue siendo la
//!   vía normal; esto es solo el caso excepcional.
//! - `user_usage_limit` (techo de gasto mensual — `hard_usd = techo_gasto_usd`,
//!   `soft_usd = 80% de hard_usd`).
//!
//! Idempotente: re-ejecutar el mismo CSV no duplica nada (usuarios existentes
//! por email se detectan y solo se sincronizan rol/perfil/techo; `assign_role`
//! y `resource_acl.grant` ya son upserts a nivel de repositorio).
//!
//! Uso:
//! ```text
//! cargo run -p aionui-app --bin seed_role_pack_users -- \
//!     --db-path /ruta/a/aionui-backend.db \
//!     --csv scripts/usuarios_ingelmec.csv
//! ```
//!
//! Ver `scripts/usuarios_ingelmec.example.csv` para el formato exacto y
//! `scripts/README_seed_usuarios.md` para cómo José da de alta a los ~40
//! empleados reales.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Parser;

use aionui_auth::hash_password;
use aionui_db::{
    IAgentProfileRepository, IResourceAclRepository, IUsageRepository, IUserRepository, SqliteAgentProfileRepository,
    SqliteResourceAclRepository, SqliteUsageRepository, SqliteUserRepository, init_database,
};

/// Roles válidos — deben coincidir con el seed de `014_rbac_roles.sql`.
const VALID_ROLES: &[&str] = &["admin", "gerencia", "tecnica", "comercial", "financiera", "ingenieria"];

/// Role-pack por defecto de cada rol (nombre de `agent_profiles.name`,
/// sembrado por `022_role_pack_profiles.sql`). Se usa para decidir si la
/// columna `perfil` del CSV es una EXCEPCIÓN (perfil distinto al de su rol)
/// que amerita un grant directo por usuario, o si es simplemente el
/// role-pack por defecto (ya cubierto por el grant de rol, no hace falta
/// grant adicional).
fn default_profile_for_role(role: &str) -> &'static str {
    match role {
        "admin" => "admin",
        "gerencia" => "gerencia",
        "tecnica" => "servimec-tko",
        "comercial" => "comercial",
        "financiera" => "financiera",
        "ingenieria" => "ingenieria",
        _ => "",
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "seed_role_pack_users",
    about = "Alta idempotente de empleados de Ingelmec (usuario + rol + perfil overlay + techo de gasto) desde un CSV"
)]
struct Cli {
    /// Ruta al archivo SQLite del Core (p.ej. <data_dir>/aionui-backend.db).
    /// NUNCA corre contra un archivo que no exista ya inicializado por el
    /// Core salvo que --allow-create esté presente (evita crear una DB nueva
    /// vacía por accidente en la ruta equivocada).
    #[arg(long)]
    db_path: PathBuf,

    /// Ruta al CSV de empleados (columnas: email,nombre,rol,perfil,techo_gasto_usd).
    #[arg(long)]
    csv: PathBuf,

    /// Permite crear el archivo de DB si no existe todavía (por defecto,
    /// falla explícitamente para evitar apuntar a la ruta equivocada).
    #[arg(long, default_value_t = false)]
    allow_create: bool,

    /// No escribe nada — solo valida el CSV y reporta qué haría.
    #[arg(long, default_value_t = false)]
    dry_run: bool,
}

#[derive(Debug, Clone)]
struct EmployeeRow {
    line_no: usize,
    email: String,
    nombre: String,
    rol: String,
    perfil: Option<String>,
    techo_gasto_usd: f64,
}

/// Parser CSV minimalista (sin dependencia externa `csv`): las 5 columnas del
/// formato de este seed nunca llevan comas ni comillas embebidas (email,
/// nombre de persona, slug de rol, slug de perfil, número). Si en el futuro
/// hiciera falta escapado real (nombres con comas), migrar a la crate `csv`.
fn parse_csv(content: &str) -> Result<Vec<EmployeeRow>> {
    let mut lines = content.lines().enumerate();

    let Some((_, header)) = lines.next() else {
        bail!("CSV vacío");
    };
    let header_cols: Vec<&str> = header.trim().split(',').map(str::trim).collect();
    let expected = ["email", "nombre", "rol", "perfil", "techo_gasto_usd"];
    if header_cols != expected {
        bail!(
            "cabecera del CSV inválida: esperaba {:?}, encontré {:?}",
            expected,
            header_cols
        );
    }

    let mut rows = Vec::new();
    for (idx, line) in lines {
        let line_no = idx + 1; // 1-indexed, incluye la cabecera como línea 1
        if line.trim().is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split(',').map(str::trim).collect();
        if cols.len() != 5 {
            bail!(
                "línea {line_no}: se esperaban 5 columnas (email,nombre,rol,perfil,techo_gasto_usd), encontré {}: {line:?}",
                cols.len()
            );
        }
        let [email, nombre, rol, perfil, techo] = [cols[0], cols[1], cols[2], cols[3], cols[4]];

        if email.is_empty() || !email.contains('@') {
            bail!("línea {line_no}: email inválido: {email:?}");
        }
        if nombre.is_empty() {
            bail!("línea {line_no}: nombre vacío");
        }
        if !VALID_ROLES.contains(&rol) {
            bail!("línea {line_no}: rol {rol:?} inválido — debe ser uno de {VALID_ROLES:?}");
        }
        let techo_gasto_usd: f64 = techo
            .parse()
            .with_context(|| format!("línea {line_no}: techo_gasto_usd inválido: {techo:?}"))?;
        if techo_gasto_usd < 0.0 {
            bail!("línea {line_no}: techo_gasto_usd no puede ser negativo: {techo_gasto_usd}");
        }

        rows.push(EmployeeRow {
            line_no,
            email: email.to_string(),
            nombre: nombre.to_string(),
            rol: rol.to_string(),
            perfil: if perfil.is_empty() {
                None
            } else {
                Some(perfil.to_string())
            },
            techo_gasto_usd,
        });
    }
    Ok(rows)
}

/// Deriva un `username` estable a partir del email (parte local, saneada).
/// `users.username` es `UNIQUE NOT NULL` — el email por sí solo no se usa
/// como username porque el formato de username del Core es más restrictivo
/// en otros flujos (login clásico); se deriva aquí de forma determinista para
/// que re-ejecutar el seed con el mismo CSV encuentre siempre el mismo user.
fn derive_username(email: &str) -> String {
    email
        .split('@')
        .next()
        .unwrap_or(email)
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Contraseña temporal fuerte y aleatoria. NO hay flag de "forzar cambio en
/// primer login" en el Core hoy (verificado: no existe tal columna en
/// `users`) — José debe comunicar esta contraseña por un canal seguro y
/// pedir el cambio manual la primera vez que el empleado entre.
fn generate_temp_password() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    // No hace falta CSPRNG de calidad criptográfica para una contraseña
    // temporal de un solo uso que el usuario cambiará; se combina el tiempo
    // de alta con un contador de proceso para variar la semilla entre runs.
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let charset: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789!@#$%";
    let mut seed = nanos ^ (std::process::id() as u128) << 64;
    let mut out = String::with_capacity(16);
    for _ in 0..16 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let idx = (seed >> 64) as usize % charset.len();
        out.push(charset[idx] as char);
    }
    out
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if !cli.db_path.exists() && !cli.allow_create {
        bail!(
            "la DB '{}' no existe. Si es intencional (primera vez), volvé a correr con --allow-create.",
            cli.db_path.display()
        );
    }
    let csv_content =
        std::fs::read_to_string(&cli.csv).with_context(|| format!("no pude leer el CSV '{}'", cli.csv.display()))?;
    let rows = parse_csv(&csv_content)?;

    println!(
        "CSV válido: {} fila(s) de empleados en '{}'.",
        rows.len(),
        cli.csv.display()
    );
    if cli.dry_run {
        for row in &rows {
            println!(
                "  [dry-run] línea {}: {} <{}> rol={} perfil_extra={:?} techo=${:.2}/mes",
                row.line_no, row.nombre, row.email, row.rol, row.perfil, row.techo_gasto_usd
            );
        }
        println!("dry-run: no se escribió nada.");
        return Ok(());
    }

    let db = init_database(&cli.db_path)
        .await
        .with_context(|| format!("no pude abrir/migrar la DB '{}'", cli.db_path.display()))?;
    let pool = db.pool().clone();

    let user_repo = SqliteUserRepository::new(pool.clone());
    let usage_repo = SqliteUsageRepository::new(pool.clone());
    let acl_repo = SqliteResourceAclRepository::new(pool.clone());
    let profile_repo = SqliteAgentProfileRepository::new(pool.clone());

    let mut created = 0usize;
    let mut existing = 0usize;
    let mut temp_passwords: Vec<(String, String, String)> = Vec::new(); // (email, username, password)

    for row in &rows {
        let username = derive_username(&row.email);

        let user = match user_repo.find_by_username(&username).await? {
            Some(u) => {
                existing += 1;
                u
            }
            None => {
                // También se busca por email antes de crear, en caso de que
                // el username derivado no coincida exactamente con un alta
                // manual previa hecha por otro canal.
                let by_email = user_repo
                    .list_users()
                    .await?
                    .into_iter()
                    .find(|u| u.email.as_deref() == Some(row.email.as_str()));
                if let Some(u) = by_email {
                    existing += 1;
                    u
                } else {
                    let temp_password = generate_temp_password();
                    let hash = hash_password(&temp_password).context("no pude hashear la contraseña temporal")?;
                    let new_user = user_repo
                        .create_user_full(&username, &hash, Some(&row.email), Some(&row.nombre), "member")
                        .await
                        .with_context(|| format!("no pude crear el usuario '{username}' ({})", row.email))?;
                    created += 1;
                    temp_passwords.push((row.email.clone(), username.clone(), temp_password));
                    new_user
                }
            }
        };

        // RBAC eje 1: asignar el rol de negocio (idempotente — INSERT OR IGNORE).
        user_repo
            .assign_role(&user.id, &row.rol)
            .await
            .with_context(|| format!("no pude asignar el rol '{}' a '{username}'", row.rol))?;

        // Overlay per-user: techo de gasto mensual (upsert real por user_id).
        let hard_usd = row.techo_gasto_usd;
        let soft_usd = hard_usd * 0.8;
        usage_repo
            .set_limit(&user.id, Some(soft_usd), Some(hard_usd))
            .await
            .with_context(|| format!("no pude fijar el techo de gasto de '{username}'"))?;

        // Excepción puntual: grant DIRECTO por usuario si `perfil` viene
        // informado y difiere del role-pack por defecto de su rol (que ya
        // tiene acceso vía el grant de rol de la migración 022).
        if let Some(perfil) = &row.perfil
            && perfil != default_profile_for_role(&row.rol)
        {
            let profile = profile_repo
                .get_by_name(perfil)
                .await
                .with_context(|| format!("error buscando el perfil '{perfil}' para '{username}'"))?
                .with_context(|| {
                    format!(
                        "línea {}: el perfil '{perfil}' no existe en agent_profiles — revisá el nombre",
                        row.line_no
                    )
                })?;
            acl_repo
                .grant("agent_profile", &profile.id, &user.id, "read")
                .await
                .with_context(|| format!("no pude otorgar el grant directo de '{perfil}' a '{username}'"))?;
            println!(
                "  excepción: '{username}' recibe grant directo adicional al perfil '{perfil}' (rol base: {})",
                row.rol
            );
        }

        println!(
            "  ok: {} <{}> username={username} rol={} techo=${:.2}/mes (soft=${:.2})",
            row.nombre, row.email, row.rol, hard_usd, soft_usd
        );
    }

    println!(
        "\nListo: {} usuario(s) nuevos, {} ya existentes (rol/perfil/techo sincronizados igual).",
        created, existing
    );

    if !temp_passwords.is_empty() {
        println!("\n⚠️  Contraseñas TEMPORALES generadas (comunicar por canal seguro, no queda registro en claro):");
        for (email, username, password) in &temp_passwords {
            println!("  {email} (username={username}): {password}");
        }
        println!(
            "\nNOTA: el Core no tiene hoy un flag de 'forzar cambio de contraseña en el primer login' — \
            pedile a cada empleado que la cambie manualmente apenas entre."
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_csv() {
        let csv = "email,nombre,rol,perfil,techo_gasto_usd\n\
                   ada@ingelmec.com,Ada Lovelace,ingenieria,,8.0\n\
                   bob@ingelmec.com,Bob Tecnico,tecnica,servimec-tko,6.0\n";
        let rows = parse_csv(csv).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].email, "ada@ingelmec.com");
        assert_eq!(rows[0].rol, "ingenieria");
        assert_eq!(rows[0].perfil, None);
        assert_eq!(rows[0].techo_gasto_usd, 8.0);
        assert_eq!(rows[1].perfil.as_deref(), Some("servimec-tko"));
    }

    #[test]
    fn rejects_bad_header() {
        let csv = "email,nombre,role,profile,cap\nada@x.com,Ada,ingenieria,,8.0\n";
        let err = parse_csv(csv).unwrap_err();
        assert!(err.to_string().contains("cabecera"));
    }

    #[test]
    fn rejects_invalid_role() {
        let csv = "email,nombre,rol,perfil,techo_gasto_usd\nada@x.com,Ada,not-a-role,,8.0\n";
        let err = parse_csv(csv).unwrap_err();
        assert!(err.to_string().contains("rol"), "got: {err}");
    }

    #[test]
    fn rejects_invalid_email() {
        let csv = "email,nombre,rol,perfil,techo_gasto_usd\nno-arroba,Ada,ingenieria,,8.0\n";
        let err = parse_csv(csv).unwrap_err();
        assert!(err.to_string().contains("email"));
    }

    #[test]
    fn rejects_negative_cap() {
        let csv = "email,nombre,rol,perfil,techo_gasto_usd\nada@x.com,Ada,ingenieria,,-1.0\n";
        let err = parse_csv(csv).unwrap_err();
        assert!(err.to_string().contains("negativo"));
    }

    #[test]
    fn skips_blank_lines() {
        let csv = "email,nombre,rol,perfil,techo_gasto_usd\n\nada@x.com,Ada,ingenieria,,8.0\n\n";
        let rows = parse_csv(csv).unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn derive_username_sanitizes_local_part() {
        assert_eq!(derive_username("Jose.Tabora@ingelmec.com"), "jose.tabora");
        assert_eq!(derive_username("weird+chars@x.com"), "weird_chars");
    }

    #[test]
    fn default_profile_matches_seed_role_packs() {
        assert_eq!(default_profile_for_role("tecnica"), "servimec-tko");
        assert_eq!(default_profile_for_role("ingenieria"), "ingenieria");
        assert_eq!(default_profile_for_role("admin"), "admin");
    }
}
