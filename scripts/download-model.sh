#!/usr/bin/env bash
# Downloads the Parakeet TDT 0.6B v3 INT8 ONNX model from HuggingFace.
#
# Pinned to a specific HuggingFace commit SHA so every install gets the
# same bytes. The repo upstream is NVIDIA's nvidia/parakeet-tdt-0.6b-v3
# (CC-BY-4.0); this is the int8-quantized ONNX conversion by istupakov.
set -euo pipefail

# Pinned model revision. Bump and re-run to upgrade.
MODEL_REPO="istupakov/parakeet-tdt-0.6b-v3-onnx"
MODEL_REV="8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce"

# The Rust side (default_model_dir() in src/main.rs) derives the model
# directory from dirs::data_dir() + "utter/models/parakeet-tdt-0.6b-v3-int8".
# On macOS that's ~/Library/Application Support/...; on Linux it's
# $XDG_DATA_HOME/... or ~/.local/share/... . Keep in lockstep with that.
case "$OSTYPE" in
    darwin*)
        DEST="$HOME/Library/Application Support/utter/models/parakeet-tdt-0.6b-v3-int8"
        ;;
    *)
        DEST="${XDG_DATA_HOME:-$HOME/.local/share}/utter/models/parakeet-tdt-0.6b-v3-int8"
        ;;
esac
HF="https://huggingface.co/${MODEL_REPO}/resolve/${MODEL_REV}"

mkdir -p "$DEST"
cd "$DEST"

FILES=(
    config.json
    vocab.txt
    nemo128.onnx
    encoder-model.int8.onnx
    decoder_joint-model.int8.onnx
)

for f in "${FILES[@]}"; do
    if [[ -s "$f" ]]; then
        echo "have $f"
        continue
    fi
    echo "fetching $f"
    curl -L --fail --progress-bar -o "$f" "$HF/$f"
done

echo
echo "model ready at: $DEST"
du -sh "$DEST"
