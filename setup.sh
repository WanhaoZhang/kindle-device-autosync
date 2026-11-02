#!/bin/sh
set -eu
TASK_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "$TASK_DIR"
python3 -c 'import sys;assert sys.version_info >= (3,11), "Require Python 3.11+"'
python3 -m venv .venv-bridge
.venv-bridge/bin/python -m pip install -r requirements.txt
printf 'Configuration helper ready. Run ./启动设备配置.command (http://127.0.0.1:8766).\n'
