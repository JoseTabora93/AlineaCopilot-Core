# Alta de empleados de Ingelmec (tarea A6 — mapeo usuario→perfil)

Este directorio trae el seed idempotente para dar de alta a los ~40 empleados
de Ingelmec, cada uno con su rol RBAC (`admin`, `gerencia`, `tecnica`,
`comercial`, `financiera`, `ingenieria`) mapeado a su role-pack de agente
(`agent_profiles`, migración `022_role_pack_profiles.sql`) y su techo de gasto
mensual (`user_usage_limit`).

**Nunca se crea un agente completo por empleado.** El modelo es role-pack
COMPARTIDO por rol; lo único que cambia por persona es su identidad, su rol,
y (opcionalmente) un grant extra a un perfil distinto del de su rol.

## 1. Los 6 role-packs (ya sembrados, no hay que crearlos)

La migración `crates/aionui-db/migrations/022_role_pack_profiles.sql` ya crea
los 6 `agent_profiles` (uno por rol del Core) fusionando cada rol con su stub
`openclaw-<rol>.md` correspondiente (tarea A5), y otorga el grant
rol→perfil en `resource_acl`:

| Rol RBAC (`roles.name`) | Perfil de agente (`agent_profiles.name`) |
|---|---|
| `admin` | `admin` |
| `gerencia` | `gerencia` |
| `tecnica` | `servimec-tko` (soporte técnico Vertiv/Liebert/TKO) |
| `comercial` | `comercial` |
| `financiera` | `financiera` |
| `ingenieria` | `ingenieria` |

Esta migración corre sola al iniciar el Core (como cualquier otra migración
`sqlx`) — no requiere ninguna acción manual.

## 2. Preparar el CSV real de los ~40 empleados

Copiá `scripts/usuarios_ingelmec.example.csv` a `scripts/usuarios_ingelmec.csv`
(este último **no se commitea** — agregalo a tu `.gitignore` local o simplemente
no lo stagees; contiene datos de personas) y reemplazá las filas de ejemplo por
los ~40 empleados reales. Columnas:

| Columna | Obligatoria | Descripción |
|---|---|---|
| `email` | sí | Email corporativo del empleado. Debe contener `@`. |
| `nombre` | sí | Nombre visible (se guarda como `display_name`). |
| `rol` | sí | Uno de los 6 roles: `admin`, `gerencia`, `tecnica`, `comercial`, `financiera`, `ingenieria`. |
| `perfil` | no | **Dejar vacío** salvo excepción puntual (ver §3). |
| `techo_gasto_usd` | sí | Techo de gasto mensual en USD (`hard_usd`; `soft_usd` se fija automáticamente al 80%). |

Ejemplo (`scripts/usuarios_ingelmec.example.csv`):

```csv
email,nombre,rol,perfil,techo_gasto_usd
ejemplo.ingeniero@ingelmec.com,Ejemplo Ingeniero,ingenieria,,8.0
ejemplo.tecnico@ingelmec.com,Ejemplo Tecnico de Campo,tecnica,,6.0
```

No inventes filas: el CSV real lo provee José con los datos verdaderos de los
~40 empleados (nombre, email, rol, techo). Este repo solo trae el ejemplo con
datos ficticios.

## 3. La columna `perfil` — solo para excepciones puntuales

Cada rol ya tiene acceso a su role-pack homónimo (tabla de arriba) por el
grant de la migración 022 — **no hace falta llenar `perfil`** en el caso normal.

Llenala SOLO cuando un empleado necesite, además de su role-pack, acceso a
OTRO perfil distinto del de su rol (p. ej. un comercial que también necesita
consultar `servimec-tko` para dar seguimiento a un caso de servicio). En ese
caso el seed otorga un grant DIRECTO por usuario (`resource_acl`,
`principal_type='user'`) sobre ese perfil adicional — la excepción puntual que
pide el plan, sin tocar el modelo de rol de nadie más.

## 4. Correr el seed

```bash
cargo run -p aionui-app --bin seed_role_pack_users -- \
    --db-path <data_dir>/aionui-backend.db \
    --csv scripts/usuarios_ingelmec.csv
```

- `<data_dir>` es el directorio de datos del Core en el servidor donde corre
  `aioncore` (donde vive `aionui-backend.db`). **Nada contra producción sin OK
  de José** — correlo primero contra una copia de la DB o en un entorno de
  prueba.
- Agregá `--dry-run` para validar el CSV (formato, roles válidos, montos) sin
  escribir nada.
- El comando es **idempotente**: volver a correrlo con el mismo CSV no
  duplica usuarios ni grants — sincroniza rol/perfil/techo de los que ya
  existen (detectados por email) y solo crea los que faltan.
- Los usuarios NUEVOS reciben una contraseña temporal aleatoria que el
  comando imprime UNA sola vez al final (no queda guardada en ningún lado en
  claro). Comunicala por un canal seguro. **El Core no tiene hoy un flag de
  "forzar cambio de contraseña en el primer login"** — pedile a cada persona
  que la cambie manualmente apenas entre.

## 5. Verificar el resultado

```bash
sqlite3 <data_dir>/aionui-backend.db \
  "SELECT u.username, u.email, ur.role_id, l.hard_usd \
   FROM users u \
   JOIN user_roles ur ON ur.user_id = u.id \
   LEFT JOIN user_usage_limit l ON l.user_id = u.id \
   ORDER BY u.username;"
```

## Qué NO hace este seed

- No crea ningún `agent_profile` nuevo (los 6 role-packs ya existen por la
  migración 022).
- No toca `acl_policy` ni la clasificación de documentos/skills.
- No asigna canales (Telegram, etc.) — eso es overlay per-user de otra tarea
  del plan.
- No fuerza cambio de contraseña en el primer login (limitación conocida del
  Core hoy).
