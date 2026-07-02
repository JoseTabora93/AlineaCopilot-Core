# PERFIL v1 — esquema de `agent_profiles.definition`

> **v1 — pendiente ratificación de José.**
> Congelado por Fable en la decisión de arquitectura A0 del plan
> `hermes-alinea-plan.md`. Este documento es la fuente de verdad del JSON que
> vive en la columna `agent_profiles.definition` (migración `020_agent_profiles.sql`).
> Los compiladores deterministas (tareas A4 → Hermes, A5 → OpenClaw, A8 →
> Copilot/assistants del propio Core) leen este JSON y lo materializan a
> config nativa de cada motor. **El perfil es DATO; el motor sigue vanilla.**

## Esquema base

```json
{
  "name": "ingenieria",
  "label": "Ingeniería (role-pack)",
  "engines": ["hermes", "openclaw"],
  "soul_md": "<system prompt completo del perfil>",
  "model": { "primary": "zai/glm-5.1", "fallbacks": ["openrouter/anthropic/claude-haiku-4-5"] },
  "mcp_allowlist": ["ingelmec-kb", "zoho-mail", "dxf-takeoff"],
  "skills": ["ingenieria/electrico", "alcance-bom"],
  "kb_scope": ["tko", "boletas"],
  "channels": [{ "type": "web" }, { "type": "telegram", "binding": "per-user" }],
  "caps": { "soft_usd": 5.0, "hard_usd": 10.0, "period": "month" },
  "acl": { "etiqueta": "interno", "roles": ["ingenieria", "admin"] }
}
```

## Campos

| Campo | Tipo | Obligatorio | Descripción |
|---|---|---|---|
| `name` | `string` | sí | Slug único y estable del perfil (kebab-case, sin espacios). Es la clave que usan los compiladores para nombrar directorios/workspaces (`~/.hermes/profiles/<name>/`, `~/.openclaw/agents/<name>/`) y la que persiste en `agent_profiles.name` (columna `UNIQUE`). Inmutable en la práctica: cambiarlo rompe el mapeo con los motores. |
| `label` | `string` | sí | Nombre visible para humanos en UIs/paneles admin. Puede cambiar libremente sin afectar a los motores. |
| `engines` | `string[]` | sí | Motores donde este perfil debe compilarse. Valores válidos: `"hermes"`, `"openclaw"`, `"copilot"`. Un perfil puede vivir en varios motores a la vez (compilación multi-destino) o en uno solo. `"copilot"` = el propio Core (`AlineaCopilot-Core`): el compilador determinista de la tarea A8 (`aionui_assistant::profile_compiler`) materializa el perfil como un `assistant` gestionado (`assistant_definitions.source='generated'`), visible en `/api/assistants` para los usuarios con grant al perfil (mismo gate de `GET /api/profiles`, tarea A1). |
| `soul_md` | `string` | sí | El system prompt completo del perfil en Markdown — la "alma" del agente. Se vuelca tal cual a `SOUL.md` (Hermes) o al frontmatter/cuerpo del agente OpenClaw (`workspace/agents/<name>.md`). |
| `model` | `object` | sí | `{ "primary": string, "fallbacks": string[] }`. `primary` es el modelo por defecto (formato `<proveedor>/<modelo>`, p.ej. `zai/glm-5.1`). `fallbacks` es la lista ordenada de modelos alternos si `primary` falla (gotcha conocido: z.ai sin saldo → fallback obligatorio a OpenRouter). |
| `mcp_allowlist` | `string[]` | sí (puede ser `[]`) | Nombres de servidores MCP que este perfil puede invocar. Alimenta los `scopes` del token `IdentityClaims` (tarea A2) — el gate de OpenClaw (tarea A3) rechaza llamadas a MCPs fuera de esta lista. |
| `skills` | `string[]` | sí (puede ser `[]`) | Skills habilitadas para el perfil, en formato `<categoria>/<skill>` (p.ej. `ingenieria/electrico`) o slug plano (p.ej. `alcance-bom`) según el catálogo de skills del workspace. |
| `kb_scope` | `string[]` | sí (puede ser `[]`) | Colecciones/particiones de la base de conocimiento (`ingelmec-kb`, FAISS/SQLite) a las que el perfil tiene acceso de lectura. Acota el RAG por perfil. |
| `channels` | `object[]` | sí (puede ser `[]`) | Canales de entrada habilitados. Cada entrada es `{ "type": "web" \| "telegram" \| ..., "binding"?: "per-user" \| "shared" }`. `binding` solo aplica a canales con identidad por usuario (p.ej. Telegram); su ausencia implica canal compartido/sin binding individual. |
| `caps` | `object` | sí | `{ "soft_usd": number, "hard_usd": number, "period": "day" \| "week" \| "month" }`. Techo de gasto del perfil — `soft_usd` dispara alerta, `hard_usd` bloquea (enforcement en `user_usage_limit` / tarea C4 para enforcement por perfil). `hard_usd` DEBE ser ≥ `soft_usd`. |
| `acl` | `object` | sí | `{ "etiqueta": string, "roles": string[] }`. `etiqueta` es la clasificación de sensibilidad del perfil mismo (reutiliza el vocabulario de `acl_policy.etiqueta`: `"publico"` \| `"interno"` \| `"confidencial-<area>"`). `roles` es la lista de roles (`roles.name` del eje 1 RBAC) que —además de grants directos por usuario— dan acceso al perfil vía `resource_acl` (resource_type=`'agent_profile'`, principal_type=`'role'`). |

### Notas de validación (aplicadas por el Core al crear/actualizar)

- El JSON se valida contra este esquema: **campos desconocidos se rechazan** con error claro (`definition contains unknown field '<x>'`), y los campos obligatorios ausentes también (`definition missing required field '<x>'`).
- `name` en el JSON debe coincidir con la columna `agent_profiles.name` de la fila (evita drift entre la clave de la tabla y el contenido).
- `engines[]` solo acepta `"hermes"` / `"openclaw"` / `"copilot"`.
- `caps.period` solo acepta `"day"` / `"week"` / `"month"`.
- `caps.hard_usd >= caps.soft_usd`.
- `acl.etiqueta` es texto libre (no hay `FOREIGN KEY` a `acl_policy` — el eje 1 y el eje 2/perfiles son ortogonales), pero se recomienda reusar el vocabulario existente.

---

## Ejemplo 1 — role-pack `ingenieria`

Role-pack compartido por todo el equipo técnico. Vive en ambos motores: en
Hermes como perfil propio (proceso dedicado), en OpenClaw como agente del
workspace de preventa/diseño.

```json
{
  "name": "ingenieria",
  "label": "Ingeniería (role-pack)",
  "engines": ["hermes", "openclaw"],
  "soul_md": "# Ingeniería Ingelmec\n\nEres el agente de ingeniería de Ingelmec (HVAC, eléctrico, incendios, datos, civil). Trabajas en español, citas solo normas de la base de conocimiento (NEC 2020, NFPA 72/13/2001, TIA-568/569/942, ASHRAE), y marcas POR CONFIRMAR en vez de inventar. Produces memorias de cálculo, BOM con rastro de cálculo y alcances de obra en formato Ingelmec (navy #1F5F8B, teal #2ABFBF, gold #F5A623).\n\n## Reglas duras\n- No inventes valores: si falta un dato del levantamiento, márcalo POR CONFIRMAR.\n- BOM y Alcance son documentos SEPARADOS (el BOM lleva costos y desperdicio; el Alcance es para cliente, sin costos).\n- Usa cálculos deterministas; nunca estimes a ojo.\n",
  "model": { "primary": "zai/glm-5.1", "fallbacks": ["openrouter/anthropic/claude-haiku-4-5"] },
  "mcp_allowlist": ["ingelmec-kb", "zoho-mail", "dxf-takeoff", "hvac-calc"],
  "skills": ["ingenieria/electrico", "ingenieria/hvac", "ingenieria/fire", "ingenieria/data", "alcance-bom"],
  "kb_scope": ["normas", "boletas", "proyectos-tecnicos"],
  "channels": [{ "type": "web" }, { "type": "telegram", "binding": "per-user" }],
  "caps": { "soft_usd": 5.0, "hard_usd": 10.0, "period": "month" },
  "acl": { "etiqueta": "interno", "roles": ["ingenieria", "admin"] }
}
```

## Ejemplo 2 — experto `servimec-tko`

Perfil especializado en el servicio técnico de mantenimiento/soporte (TKO =
takeoff/soporte de campo ServiMec). Solo Hermes (perfil único hoy en
`hermes.ingelmec.ai`, Box A), acceso restringido al equipo técnico.

```json
{
  "name": "servimec-tko",
  "label": "ServiMec — Soporte técnico TKO",
  "engines": ["hermes"],
  "soul_md": "# ServiMec TKO\n\nEres el asistente de soporte técnico de ServiMec. Respondes consultas de campo sobre equipos instalados (boletas de servicio, tomas de datos históricas) usando la base de conocimiento de 95K fragmentos + 11K boletas. Español, directo, cita la boleta/fuente exacta cuando exista. Si no hay evidencia en la base, dilo explícitamente — no adivines número de serie, modelo ni fecha.\n",
  "model": { "primary": "zai/glm-5.1", "fallbacks": ["openrouter/anthropic/claude-haiku-4-5", "openrouter/qwen/qwen3-coder"] },
  "mcp_allowlist": ["ingelmec-kb"],
  "skills": [],
  "kb_scope": ["boletas", "tko"],
  "channels": [{ "type": "telegram", "binding": "per-user" }],
  "caps": { "soft_usd": 3.0, "hard_usd": 6.0, "period": "month" },
  "acl": { "etiqueta": "interno", "roles": ["tecnica", "admin"] }
}
```

## Ejemplo 3 — `preventa`

Orquesta el pipeline determinista de preventa MEP (`preventa_pipeline.sh`) vía
OpenClaw. Cap de gasto ajustado al costo real medido por `preventa_cost.py`.

```json
{
  "name": "preventa",
  "label": "Preventa MEP",
  "engines": ["openclaw"],
  "soul_md": "# Preventa MEP\n\nEres el orquestador de preventa de Ingelmec. Conduces el pipeline determinista (levantamiento → cálculo → BOM/Alcances → propuesta) sobre proyectos MEP (HVAC/eléctrico/incendios/datos/civil). No mides cantidades tú mismo cuando existan mediciones deterministas (`mediciones_crudas.json`, tarea B1) — las consumes y clasificas/auditas. Produces datos (bom_data.json, alcance_data.json); el código determinista arma el documento (ingelmec_gen.py, builders de jetos). Español, formato Ingelmec.\n",
  "model": { "primary": "zai/glm-5.1", "fallbacks": ["openrouter/anthropic/claude-haiku-4-5"] },
  "mcp_allowlist": ["ingelmec-kb", "zoho-mail", "dxf-takeoff", "docgen"],
  "skills": ["alcance-bom", "ingenieria/electrico", "ingenieria/hvac"],
  "kb_scope": ["normas", "biblioteca-lineas", "proyectos-comerciales"],
  "channels": [{ "type": "web" }],
  "caps": { "soft_usd": 4.0, "hard_usd": 8.0, "period": "month" },
  "acl": { "etiqueta": "interno", "roles": ["comercial", "tecnica", "admin"] }
}
```
