#!/bin/sh
set -eu

destination=${1:?Usage: copy_package_assets.sh DESTINATION [PLATFORM]}

rm -rf "$destination"
mkdir -p "$destination"
if [ -d .artifacts/boot-assets ]; then
    cp -RL .artifacts/boot-assets/. "$destination/"
fi
python3 - "$destination" <<'PY'
import pathlib, sys
root = pathlib.Path(sys.argv[1])
for path in root.glob('bootah-fetch*'):
    if path.is_file(): path.unlink()
for name in ['7zz', '7zzs', '7z.exe', '7z.dll']:
    (root / name).unlink(missing_ok=True)
for name in ['License.txt', 'copying.txt', 'source.json']:
    path = root / name
    if path.is_file() and any(marker in path.read_bytes().lower() for marker in [b'7-zip', b'ip7z/7zip']):
        path.unlink()
PY
