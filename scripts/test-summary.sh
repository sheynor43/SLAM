#!/usr/bin/env bash
# Запуск тестов с коротким выводом: только упавшие тесты и итог.
# Полный лог сохраняется в target/test-output.log.
set -uo pipefail
mkdir -p target
LOG=target/test-output.log

if command -v cargo-nextest &>/dev/null; then
  cargo nextest run --workspace --no-fail-fast "$@" >"$LOG" 2>&1
else
  cargo test --workspace --no-fail-fast "$@" >"$LOG" 2>&1
fi
STATUS=$?

if [[ $STATUS -eq 0 ]]; then
  echo "OK: все тесты прошли ($(grep -Eo '[0-9]+ (tests? )?passed' "$LOG" | tail -n1))"
else
  echo "FAIL: есть ошибки. Ключевые строки:"
  grep -E "FAIL|panicked|^error(\[|:)|failures:" "$LOG" | head -n 60
  echo "Полный лог: $LOG"
fi
exit $STATUS
