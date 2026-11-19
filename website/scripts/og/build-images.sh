#!/usr/bin/env bash
# Regenerates public/og-image.png (the picture shown when the site is shared) and the icon
# set. Needs Google Chrome and rsvg-convert (brew install librsvg). Run after `npm install`,
# since og.html reads the fonts from node_modules.
set -euo pipefail
cd "$(dirname "$0")/../.."
CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
"$CHROME" --headless=new --disable-gpu --hide-scrollbars --force-device-scale-factor=1 \
  --window-size=1200,630 --virtual-time-budget=3000 \
  --screenshot="$PWD/public/og-image.png" "file://$PWD/scripts/og/og.html" >/dev/null 2>&1
WORK="$(mktemp -d)"
# App icons: the mark on a full-bleed dark tile, so a round or squircle mask cuts nothing off.
cat > "$WORK/icon.svg" <<'SVG'
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><rect width="64" height="64" fill="#14120f"/><g transform="translate(8 8) scale(0.75)"><circle cx="21" cy="30" r="13" fill="#f4eedd"/><circle cx="43" cy="30" r="13" fill="#f4eedd"/><circle cx="23" cy="31" r="5.5" fill="#14120f"/><circle cx="45" cy="31" r="5.5" fill="#14120f"/><path d="M32 40l-5 7 5 6 5-6z" fill="#f2c230"/></g></svg>
SVG
rsvg-convert -w 180 -h 180 "$WORK/icon.svg" -o public/apple-touch-icon.png
rsvg-convert -w 192 -h 192 "$WORK/icon.svg" -o public/icon-192.png
rsvg-convert -w 512 -h 512 "$WORK/icon.svg" -o public/icon-512.png
rsvg-convert -w 512 -h 512 public/favicon.svg -o public/logo.png
rsvg-convert -w 48 -h 48 public/favicon.svg -o "$WORK/fav48.png"
python3 -c "import struct,sys;p=open(sys.argv[1],'rb').read();open('public/favicon.ico','wb').write(struct.pack('<HHHBBBBHHII',0,1,1,48,48,0,0,1,32,len(p),22)+p)" "$WORK/fav48.png"
rm -rf "$WORK"
echo "images built"
