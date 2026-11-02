#!/bin/sh
set -eu
TASK_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
exec "$TASK_DIR/.venv-bridge/bin/python" "$TASK_DIR/device_app.py" --open-browser "$@"
