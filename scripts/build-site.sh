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
odd=$(find "$root/site" ! -path "$root/site" -type d | head -n 3)
[ -z "$odd" ] || die "site/ holds a folder: $odd"
# No folder on the way to these files is a link either.
real=$(CDPATH='' cd -- "$root" && pwd -P)
for d in site docs/img assets/fonts/ibm-plex-sans assets/fonts/ibm-plex-mono \
    assets/icons/ui/design/svg/model assets/icons/app; do
    [ "$(CDPATH='' cd -- "$root/$d" && pwd -P)" = "$real/$d" ] || die "$d is reached through a link."
done

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
for p in "$out"/img/*.png; do
    [ "$(head -c 8 "$p" | od -An -tx1 | tr -d ' \n')" = 89504e470d0a1a0a ] || die "img/${p##*/} is not a PNG picture."
done

# Only these files are published. A page or a drawing anywhere else (a sub-folder of site/,
# a .htm or .xhtml file) would be served without any of the checks below.
odd=$(cd "$out" && find . -type f ! -name "$mark" |
    grep -Evx '\./(index\.html|404\.html|style\.css|site\.js|icon\.svg|icon-256\.png|favicon\.ico|img/[A-Za-z0-9_-]+\.png|fonts/(IBMPlexSans-Variable\.ttf|IBMPlexMono-Regular\.ttf|OFL\.txt))' |
    head -n 3 | tr '\n' ' ')
[ -z "$odd" ] || die "not on the list of files this script publishes: $odd"

# A drawing opened by its own address is a page of its own, and the policy line of index.html
# does not reach it. So a drawing holds plain shapes and nothing else: ASCII text, tags from a
# short list, no attribute that starts with "on", no link, no style. A search for "<script"
# alone is not enough: a browser also runs <s:script>, a tag built from an entity, an
# attribute at the start of a line and a file in UTF-16.
for svg in "$out"/*.svg; do
    [ -f "$svg" ] || continue
    name=${svg##*/}
    # What is left when printable ASCII and line ends are taken out: UTF-16, tabs, hidden characters.
    [ "$(LC_ALL=C tr -d ' -~\n' <"$svg" | wc -c | tr -d ' ')" -eq 0 ] ||
        die "$name holds something that is not plain ASCII text."
    flat=$(tr '\n' ' ' <"$svg")
    case $flat in
        *'<!'* | *'<?'* | *'&'*) die "$name holds a DOCTYPE, a comment, a processing instruction or an entity." ;;
    esac
    odd=$(printf '%s\n' "$flat" | grep -Eo '<[^/ >][^ />]*' | sort -u |
        grep -Evx '<(svg|title|desc|g|defs|path|circle|ellipse|rect|line|polyline|polygon|linearGradient|radialGradient|stop|clipPath)' |
        tr '\n' ' ')
    [ -z "$odd" ] || die "$name holds tags a plain drawing does not need: $odd"
    if printf '%s\n' "$flat" | grep -Eio "([[:space:]\"']on[a-z]+|href|style)[[:space:]]*=|javascript:|url\\([[:space:]]*[^#[:space:]]"; then
        die "$name holds a handler, a link, a style or an address (see the lines above)."
    fi
done

# Each page carries a Content-Security-Policy line that allows the page's own files and nothing
# else. A browser holds the page to it: nothing is fetched from another host and no script or
# style inside the HTML runs, however it is written. The searches further down say the same
# early and by line; they do not read site.js, and the policy line is what counts.
#
# So each page begins with the same six lines, and they are compared byte for byte. A browser
# drops a policy line that does not stand in the <head>, and little is needed for that: a
# letter or a tag before it, a missing quote in the line above, a character nobody sees. A
# policy that only looks strict (a host name without https://, a nonce, a hash) is refused the
# same way. To change the policy, change it here first. 404.html runs no script.
start='<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">'
own="style-src 'self'; img-src 'self'; font-src 'self'; base-uri 'none'; form-action 'none'"
for page in "$out"/*.html; do
    name=${page##*/}
    case $name in
        index.html) policy="default-src 'none'; script-src 'self'; $own; require-trusted-types-for 'script'; trusted-types 'none'" ;;
        *) policy="default-src 'none'; $own" ;;
    esac
    printf '%s\n<meta http-equiv="Content-Security-Policy" content="%s">\n' "$start" "$policy" >"$out/.head"
    head -n 6 "$page" | cmp -s - "$out/.head" ||
        die "$name does not begin with the six lines this script knows: doctype, <html>, <head>, charset, viewport and the Content-Security-Policy line, which must be: $policy"
    # One line on one line: an attribute or a tag may be written over several.
    flat=$(tr '\n\r\t' '   ' <"$page")
    # The policy line is the only http-equiv: a "refresh" would send the visitor elsewhere, and
    # no policy decides where a page goes next.
    [ "$(printf '%s\n' "$flat" | grep -Eio 'http-equiv|<base[[:space:]>/]' | wc -l | tr -d ' ')" -eq 1 ] ||
        die "$name has an http-equiv besides the policy line, or a <base>. Neither belongs on the page."
    # The policy allows every file of the page's own host as script. The page has one.
    if printf '%s\n' "$flat" | grep -Eio '<script[^>]*>?' | grep -vx '<script src="site.js">'; then
        die "$name has a <script> that is not exactly <script src=\"site.js\"> (see the lines above)."
    fi
done
rm -f "$out/.head"

# Nothing on the page may be fetched from another host: no script, picture, font or style
# sheet. Ordinary links to other sites are fine.
if grep -Ein "(src|poster|data)[[:space:]]*=[[:space:]]*[\"']?[[:space:]]*(https?:)?[/\\][/\\]|srcset[[:space:]]*=[[:space:]]*[\"']?[^\"'>]*(https?:)?[/\\][/\\]" "$out"/*.html; then
    die "the page fetches something from another host (see the lines above)."
fi
for page in "$out"/*.html; do
    # On one line first: a <link> may be written over several.
    if tr '\n' ' ' <"$page" | grep -Eio '<link[^>]*>' |
        grep -vFx '<link rel="canonical" href="https://darkburn.phobic.no/">' |
        grep -Ei "href[[:space:]]*=[[:space:]]*[\"']?[[:space:]]*(https?:)?[/\\][/\\]"; then
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
if grep -Ein "<script[[:space:]]*>|<script[[:space:]][^>]*>[^<]|<style|[[:space:]]style[[:space:]]*=|javascript:" "$out"/*.html; then
    die "script or style inside the HTML (see the lines above). Put it in site.js or style.css."
fi
for page in "$out"/*.html; do
    # On one line first: an attribute may stand on a line of its own.
    if tr '\n' ' ' <"$page" | grep -Eio "<[a-z][^>]*[[:space:]\"'/]on[a-z]+[[:space:]]*=[^>]{0,40}"; then
        die "${page##*/}: a handler inside a tag (see the lines above). Put it in site.js."
    fi
done

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
