#!/usr/bin/env bash
#
# Backfill de segregación de ficheros por usuario (Alinea Fase 2 #5).
#
# Cuando se activa `enforce_file_scope` (multiusuario), los workspaces NUEVOS se
# crean bajo `{work_dir}/users/{user_id}/conversations/...`. Las conversaciones
# EXISTENTES (creadas antes de activar la segregación, p. ej. una instancia
# desktop que pasa a multiusuario) tienen su workspace en el layout viejo
# `{work_dir}/conversations/...`, que el guard por-subárbol rechazaría.
#
# Este script migra los workspaces existentes al layout namespaced y actualiza
# `extra.workspace` (vía json_set sobre la clave $.workspace, no REPLACE textual)
# en la DB, mapeando conversación → dueño (user_id) desde la tabla `conversations`.
#
# Idempotente: omite los que ya estén migrados o sin user_id resoluble.
# Hacer SIEMPRE un backup de `{work_dir}` y de la DB antes de ejecutar.
#
# Uso:
#   scripts/backfill_file_scope.sh <work_dir> <sqlite_db_path> [--dry-run]
#
set -euo pipefail

WORK_DIR="${1:?uso: backfill_file_scope.sh <work_dir> <sqlite_db_path> [--dry-run]}"
DB="${2:?falta la ruta de la DB SQLite}"
DRY_RUN="${3:-}"

CONV_DIR="$WORK_DIR/conversations"
if [ ! -d "$CONV_DIR" ]; then
  echo "No existe '$CONV_DIR' — nada que migrar."
  exit 0
fi
command -v sqlite3 >/dev/null || { echo "sqlite3 no está instalado"; exit 1; }

# Escapa comillas simples para SQL (' -> '').
sql_quote() { printf "%s" "$1" | sed "s/'/''/g"; }

dry() { [ "$DRY_RUN" = "--dry-run" ]; }

migrated=0
skipped=0
for dir in "$CONV_DIR"/*/; do
  [ -d "$dir" ] || continue
  name="$(basename "$dir")"
  # El nombre del workspace es `{label}-temp-{conversation_id}`.
  conv_id="${name##*-temp-}"
  if [ "$conv_id" = "$name" ]; then
    echo "skip '$name' (no matchea el patrón *-temp-<id>)"; skipped=$((skipped + 1)); continue
  fi

  conv_q="$(sql_quote "$conv_id")"
  user_id="$(sqlite3 "$DB" "SELECT user_id FROM conversations WHERE id = '$conv_q' LIMIT 1;")"
  if [ -z "$user_id" ]; then
    echo "skip '$name' (sin user_id en la DB para conv '$conv_id')"; skipped=$((skipped + 1)); continue
  fi

  dest_dir="$WORK_DIR/users/$user_id/conversations"
  old_path="$CONV_DIR/$name"
  new_path="$dest_dir/$name"
  old_q="$(sql_quote "$old_path")"
  new_q="$(sql_quote "$new_path")"
  # Solo actualiza extra.workspace si justamente apuntaba al path viejo.
  update_sql="UPDATE conversations SET extra = json_set(extra, '\$.workspace', '$new_q') WHERE id = '$conv_q' AND json_extract(extra, '\$.workspace') = '$old_q';"

  if dry; then
    echo "[dry-run] mkdir -p '$dest_dir'"
    echo "[dry-run] mv '$old_path' -> '$new_path'"
    echo "[dry-run] sqlite: $update_sql"
  else
    mkdir -p "$dest_dir"
    mv "$old_path" "$new_path"
    sqlite3 "$DB" "$update_sql"
  fi
  echo "migrado '$name' -> users/$user_id/conversations/"
  migrated=$((migrated + 1))
done

echo "Backfill completo: $migrated migrados, $skipped omitidos."
