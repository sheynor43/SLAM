#!/usr/bin/env bash
# Установка окружения разработки SLAM на Arch Linux.
# Использование:
#   ./scripts/setup-arch.sh          — установить всё недостающее (нужен sudo)
#   ./scripts/setup-arch.sh --check  — только проверить, ничего не устанавливая
set -euo pipefail

CHECK_ONLY=0
[[ "${1:-}" == "--check" ]] && CHECK_ONLY=1

# Обязательные пакеты
REQUIRED=(
  base-devel git git-lfs github-cli rustup clang cmake pkgconf just
  sdl3 pipewire alsa-lib mesa libglvnd zip unzip
)
# Нужны позже (M0+ Vulkan, M4 lazer-export, профилировщик) — не критично
OPTIONAL=(vulkan-icd-loader vulkan-headers dotnet-sdk tracy)
# Cargo-инструменты: сначала пробуем пакет pacman, иначе cargo install
CARGO_TOOLS=(cargo-deny cargo-nextest)

missing=()
for p in "${REQUIRED[@]}"; do
  pacman -Qi "$p" &>/dev/null || missing+=("$p")
done
missing_opt=()
for p in "${OPTIONAL[@]}"; do
  pacman -Qi "$p" &>/dev/null || missing_opt+=("$p")
done
missing_tools=()
for t in "${CARGO_TOOLS[@]}"; do
  command -v "$t" &>/dev/null || missing_tools+=("$t")
done

# rustup "provides" rust, so check the actual installed package name.
if [[ "$(pacman -Qq rust 2>/dev/null)" == "rust" ]]; then
  echo "ВНИМАНИЕ: установлен пакет 'rust', он конфликтует с 'rustup'."
  echo "Удалите его: sudo pacman -Rns rust"
  [[ $CHECK_ONLY -eq 1 ]] || exit 1
fi

echo "Недостающие обязательные пакеты: ${missing[*]:-нет}"
echo "Недостающие опциональные пакеты: ${missing_opt[*]:-нет}"
echo "Недостающие cargo-инструменты:   ${missing_tools[*]:-нет}"
command -v rustc &>/dev/null && echo "rustc: $(rustc --version)" || echo "rustc: не найден"
gh auth status &>/dev/null && echo "gh: авторизован" || echo "gh: НЕ авторизован (выполните: gh auth login)"

if [[ $CHECK_ONLY -eq 1 ]]; then
  [[ ${#missing[@]} -eq 0 && ${#missing_tools[@]} -eq 0 ]] && exit 0 || exit 2
fi

if [[ ${#missing[@]} -gt 0 ]]; then
  sudo pacman -S --needed --noconfirm "${missing[@]}"
fi

# Опциональные: ставим только то, что есть в репозиториях
for p in "${missing_opt[@]}"; do
  if pacman -Si "$p" &>/dev/null; then
    sudo pacman -S --needed --noconfirm "$p" || echo "Не удалось установить $p — пропускаю"
  else
    echo "Пакета $p нет в репозиториях — пропускаю"
  fi
done

rustup default stable
rustup component add clippy rustfmt

for t in "${missing_tools[@]}"; do
  if pacman -Si "$t" &>/dev/null; then
    sudo pacman -S --needed --noconfirm "$t"
  else
    cargo install --locked "$t"
  fi
done

git lfs install

echo
echo "Готово. Если gh не авторизован — выполните: gh auth login"
