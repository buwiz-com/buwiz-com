#!/usr/bin/env bash
# The OCR models for a build with `--features ocr`: PaddleOCR PP-OCRv5
# mobile text detection and Latin-script recognition, as ONNX, with the
# recognition dictionary. About 13 MB. Each file is pinned to a commit of
# ModelScope's greatv/oar-ocr (the ONNX exports `oar-ocr` itself downloads)
# and checked against its SHA-256. PaddleOCR's models are Apache-2.0.
#
#   scripts/ocr-models.sh               # into ./ocr-models (OCR_MODEL_DIR's default)
#   scripts/ocr-models.sh /some/dir     # then OCR_MODEL_DIR=/some/dir
#
# The Dockerfile runs this; files already present and correct are kept.
set -euo pipefail
dir="${1:-ocr-models}"
mkdir -p "$dir"

# name  commit  sha256
models="
pp-ocrv5_mobile_det.onnx        41eaa7fa91fdb46de50a7a9f91e0e4f63a4ac53f 1eb7b4f7ab657ebd1c66d5f79bca7497f29768a2e3c15e52daecbba1a8e4a039
latin_pp-ocrv5_mobile_rec.onnx  7adc8404198f33a0d917be6ef603204d7f6e2d65 e3a6bfeea1c8a01d6fccfd480a0bd363fd907f8c65931e228bb2736f5c3e142f
ppocrv5_latin_dict.txt          679aeeb4b5fb29a04ff7fc9012724022ef094fb5 ccbcc45730b3fbbd9050c5bc74db6a99067141ef1035e3d14889a84a6b9b1aff
"

sha256() {
  if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

echo "$models" | while read -r name commit sum; do
  [ -n "$name" ] || continue
  target="$dir/$name"
  if [ -f "$target" ] && [ "$(sha256 "$target")" = "$sum" ]; then
    continue
  fi
  curl -fsSL --retry 3 -o "$target.part" \
    "https://www.modelscope.cn/api/v1/models/greatv/oar-ocr/repo?Revision=$commit&FilePath=$name"
  if [ "$(sha256 "$target.part")" != "$sum" ]; then
    rm -f "$target.part"
    echo "checksum mismatch for $name" >&2
    exit 1
  fi
  mv "$target.part" "$target"
  echo "$name"
done
