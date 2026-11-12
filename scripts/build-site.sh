#!/bin/sh
# Puts the web page (https://darkburn.phobic.no) together in one folder.
#
#   sh scripts/build-site.sh [folder]        the default folder is target/site
#
# The page itself is site/. This script adds what the page shows and the repository already
# holds, so that nothing is kept twice: the pictures of the README (docs/img), the two fonts of
# the program with their licence text (assets/fonts) and the icon (assets/icons). Every file is
# a plain copy. Nothing is downloaded and nothing is changed.
#
# Then it checks the result: every file the page points to is there, and nothing on the page
# is fetched from another host.
#
# To look at the page:
#   sh scripts/build-site.sh && python3 -m http.server -d target/site
# .github/workflows/pages.yml runs this script and publishes the folder.
set -eu

die() {
    printf 'build-site: %s\n' "$1" >&2
    exit 1
}

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
out=${1:-"$root/target/site"}
case $out in
    /*) ;;
    *) out=$PWD/$out ;;
esac

[ -f "$root/site/index.html" ] || die "site/index.html is not there."

# The folder is made new each time. Only a folder this script made is ever removed.
mark=.made-by-build-site
if [ -e "$out" ]; then
    if [ -f "$out/$mark" ]; then
        rm -rf "$out"
    elif [ ! -d "$out" ] || [ -n "$(ls -A "$out")" ]; then
        die "$out is there already and this script did not make it. Name another folder."
    fi
fi
mkdir -p "$out/img" "$out/fonts"
: >"$out/$mark"

cp -R "$root/site/." "$out/"
cp "$root"/docs/img/*.png "$out/img/"
cp "$root/assets/fonts/ibm-plex-sans/IBMPlexSans-Variable.ttf" "$out/fonts/"
cp "$root/assets/fonts/ibm-plex-mono/IBMPlexMono-Regular.ttf" "$out/fonts/"
# One licence text for both fonts: the two OFL.txt files are the same.
cp "$root/assets/fonts/ibm-plex-sans/OFL.txt" "$out/fonts/OFL.txt"
cp "$root/assets/icons/ui/design/svg/model/db_app.svg" "$out/icon.svg"
cp "$root/assets/icons/app/darkburn-256.png" "$out/icon-256.png"
cp "$root/assets/icons/app/darkburn.ico" "$out/favicon.ico"

# Nothing on the page may be fetched from another host: no script, picture, font or style
# sheet. Ordinary links to other sites are fine.
if grep -En '(src|poster)="(https?:)?//|srcset="[^"]*(https?:)?//' "$out"/*.html; then
    die "the page fetches something from another host (see the lines above)."
fi
if grep -Eo '<link[^>]*>' "$out"/*.html | grep -v 'rel="canonical"' | grep -E 'href="(https?:)?//'; then
    die "the page loads a file from another host with <link> (see the lines above)."
fi
for css in "$out"/*.css; do
    [ -f "$css" ] || continue
    if grep -En "@import|url\\([\"']?(https?:)?//" "$css"; then
        die "$css fetches something from another host (see the lines above)."
    fi
done

# Every file the page points to is there.
refs=$out/.refs
: >"$refs"
for page in "$out"/*.html; do
    grep -Eo '(src|href)="[^"]*"' "$page" | sed 's/^[a-z]*="//; s/"$//' >>"$refs" || true
done
for css in "$out"/*.css; do
    [ -f "$css" ] || continue
    grep -Eo 'url\([^)]*\)' "$css" | sed "s/^url(//; s/)\$//; s/^[\"']//; s/[\"']\$//" >>"$refs" || true
done
missing=0
while IFS= read -r ref; do
    case $ref in
        '' | '#'* | http://* | https://* | mailto:* | data:*) continue ;;
    esac
    file=${ref%%[?#]*}
    if [ ! -e "$out/$file" ]; then
        printf 'build-site: the page points to %s, which is not there.\n' "$ref" >&2
        missing=1
    fi
done <"$refs"
rm -f "$refs"
[ "$missing" -eq 0 ] || die "see the lines above."

printf 'The page is in %s (%s files).\n' "$out" "$(find "$out" -type f ! -name "$mark" | wc -l | tr -d ' ')"
