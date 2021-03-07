#!/bin/bash

exec "$RD_AGENT_BIN" bandit-mem-hog --wbps "$1" --rbps "$2" --report report.json
