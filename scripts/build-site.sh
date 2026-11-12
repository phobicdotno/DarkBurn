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
# Then it checks the result: every file the page points to is there, nothing on the page is
# fetched from another host, and every page carries its Content-Security-Policy line.
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

# Only plain files are published. A link could point at a file outside the repository, and what
# packs the folder for GitHub Pages puts the file itself in the place of a link.
fonts=$root/assets/fonts
icons=$root/assets/icons
for f in "$root/site" "$root/docs/img" "$root"/docs/img/*.png \
    "$fonts/ibm-plex-sans/IBMPlexSans-Variable.ttf" "$fonts/ibm-plex-mono/IBMPlexMono-Regular.ttf" \
    "$fonts/ibm-plex-sans/OFL.txt" "$icons/ui/design/svg/model/db_app.svg" \
    "$icons/app/darkburn-256.png" "$icons/app/darkburn.ico"; do
    [ ! -L "$f" ] || die "$f is a link. Only plain files are published."
done
odd=$(find "$root/site" ! -type f ! -type d | head -n 3)
[ -z "$odd" ] || die "site/ holds something that is not a plain file: $odd"

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
cp "$fonts/ibm-plex-sans/IBMPlexSans-Variable.ttf" "$out/fonts/"
cp "$fonts/ibm-plex-mono/IBMPlexMono-Regular.ttf" "$out/fonts/"
# One licence text for both fonts: the two OFL.txt files are the same.
cp "$fonts/ibm-plex-sans/OFL.txt" "$out/fonts/OFL.txt"
cp "$icons/ui/design/svg/model/db_app.svg" "$out/icon.svg"
cp "$icons/app/darkburn-256.png" "$out/icon-256.png"
cp "$icons/app/darkburn.ico" "$out/favicon.ico"

# A drawing opened by its own address is a page of its own, and the policy line of index.html
# does not reach it. So no drawing may carry script.
for svg in "$out"/*.svg "$out"/img/*.svg; do
    [ -f "$svg" ] || continue
    if grep -Ein "<script|<foreignObject|<iframe|<embed|<object|javascript:|[[:space:]]on[a-z]+[[:space:]]*=" "$svg"; then
        die "${svg##*/} carries script or another document (see the lines above)."
    fi
done

# Each page carries a Content-Security-Policy line that allows the page's own files and nothing
# else, and the line stands before anything the page loads. A browser holds the page to it:
# nothing is fetched from another host and no script or style inside the HTML runs, however it
# is written. The searches further down say the same early and by line; they do not read
# site.js, and the policy line is what counts.
for page in "$out"/*.html; do
    name=${page##*/}
    policy=$(sed -n 's/.*<meta http-equiv="Content-Security-Policy" content="\([^"]*\)">.*/\1/p' "$page")
    case $policy in
        "default-src 'none';"*) ;;
        *) die "$name has no Content-Security-Policy line that begins with default-src 'none'." ;;
    esac
    case $policy in
        *unsafe* | *'*'* | *http* | *data:* | *blob:* | *filesystem:*)
            die "$name: its Content-Security-Policy line allows more than the page's own files."
            ;;
    esac
    at=$(grep -n 'http-equiv="Content-Security-Policy"' "$page" | head -n 1 | cut -d: -f1)
    first=$(grep -Ein '<(link|script|style|img|base|body)[[:space:]>]' "$page" | head -n 1 | cut -d: -f1)
    [ "$at" -lt "${first:-0}" ] ||
        die "$name: the Content-Security-Policy line must stand before the first <link>, <script> or picture."
done

# Nothing on the page may be fetched from another host: no script, picture, font or style
# sheet. Ordinary links to other sites are fine.
if grep -Ein "(src|poster|data)[[:space:]]*=[[:space:]]*[\"']?[[:space:]]*(https?:)?//|srcset[[:space:]]*=[[:space:]]*[\"']?[^\"'>]*(https?:)?//" "$out"/*.html; then
    die "the page fetches something from another host (see the lines above)."
fi
for page in "$out"/*.html; do
    # On one line first: a <link> may be written over several.
    if tr '\n' ' ' <"$page" | grep -Eio '<link[^>]*>' | grep -vi 'rel="canonical"' |
        grep -Ei "href[[:space:]]*=[[:space:]]*[\"']?[[:space:]]*(https?:)?//"; then
        die "${page##*/} loads a file from another host with <link> (see the lines above)."
    fi
done
for css in "$out"/*.css; do
    [ -f "$css" ] || continue
    if grep -Ein "@import|url\\([[:space:]]*[\"']?[[:space:]]*(https?:)?//|[\"'][[:space:]]*(https?:)?//" "$css"; then
        die "${css##*/} fetches something from another host (see the lines above)."
    fi
done

# No script or style inside the HTML: the policy line allows only files, so a browser would
# refuse them.
if grep -Ein "<script[[:space:]]*>|<script[[:space:]][^>]*>[^<]|<style|[[:space:]]style[[:space:]]*=|[[:space:]]on[a-z]+[[:space:]]*=[[:space:]]*[\"']|javascript:" "$out"/*.html; then
    die "script or style inside the HTML (see the lines above). Put it in site.js or style.css."
fi

# Every file the page points to is there.
refs=$out/.refs
missing=0
# check_refs FILE: the addresses in $refs were found in FILE.
check_refs() {
    while IFS= read -r ref; do
        case $ref in
            '' | '#'* | http://* | https://* | mailto:* | data:*) continue ;;
            *..*)
                printf 'build-site: %s points to %s, which is outside the folder of the page.\n' "${1##*/}" "$ref" >&2
                missing=1
                continue
                ;;
            /*)
                # 404.html needs such addresses: GitHub shows that page at any depth.
                if [ "${1##*/}" != 404.html ]; then
                    printf 'build-site: %s points to %s. An address that starts with "/" breaks under phobicdotno.github.io/DarkBurn/.\n' "${1##*/}" "$ref" >&2
                    missing=1
                    continue
                fi
                ;;
        esac
        file=${ref%%[?#]*}
        if [ ! -f "$out/$file" ] && [ ! -f "$out/$file/index.html" ]; then
            printf 'build-site: %s points to %s, which is not there.\n' "${1##*/}" "$ref" >&2
            missing=1
        fi
    done <"$refs"
}
for page in "$out"/*.html; do
    {
        grep -Eio "(src|href|poster)[[:space:]]*=[[:space:]]*(\"[^\"]*\"|'[^']*')" "$page" |
            sed "s/^[^=]*=[[:space:]]*.//; s/.\$//"
        # srcset: a list with commas between; each entry is an address and then a size.
        grep -Eio "srcset[[:space:]]*=[[:space:]]*(\"[^\"]*\"|'[^']*')" "$page" |
            sed "s/^[^=]*=[[:space:]]*.//; s/.\$//" | tr ',' '\n' | sed 's/^[[:space:]]*//; s/[[:space:]].*$//'
        # The picture that other sites show for a link to the page.
        sed -n 's/.*property="og:image" content="https:\/\/darkburn\.phobic\.no\/\([^"]*\)".*/\1/p' "$page"
    } >"$refs" || true
    check_refs "$page"
done
for css in "$out"/*.css; do
    [ -f "$css" ] || continue
    grep -Eio 'url\([^)]*\)' "$css" |
        sed "s/^[^(]*([[:space:]]*//; s/[[:space:]]*)\$//; s/^[\"']//; s/[\"']\$//" >"$refs" || true
    check_refs "$css"
done
rm -f "$refs"
[ "$missing" -eq 0 ] || die "see the lines above."

printf 'The page is in %s (%s files).\n' "$out" "$(find "$out" -type f ! -name "$mark" | wc -l | tr -d ' ')"
