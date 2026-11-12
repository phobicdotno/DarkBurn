#!/bin/sh
# Puts the web page, https://darkburn.phobic.no, on its host.
#
#   sh scripts/deploy-site.sh USER
#
# USER is the SFTP user of the web host. sftp asks for the password, or uses your key; give a
# key a passphrase. The password is typed each time: it is in no file here and in no workflow,
# because the same login reaches every other site on that host.
#
# The first time, sftp shows the key of the host and asks whether to go on. Compare its
# fingerprint with the one the host's control panel shows. A computer that has uploaded to
# that host before is not asked. If it is asked all the same, or sftp says that the key has
# changed, stop: whoever answers in the host's place gets the password, and with it every
# site.
#
# Run this from main, never from a branch you have not read: this script is part of what a
# branch can change, and it runs where the password is typed. For the real page the script
# itself refuses any state but main with nothing uncommitted.
#
# The page is hosted the way the other pages on phobic.no are: a folder of plain files on the
# web host, uploaded over SFTP, with an .htaccess file in it for the headers, the "not found"
# page and https. This script
#   1. builds the folder afresh with scripts/build-site.sh, which also checks it,
#   2. uploads every file to httpd.www/darkburn/ on ssh.phobic.no under a second name
#      (x.new) and then gives each its place, the pages last. A visitor is never sent half a
#      file, and an upload that stops before that leaves the page as it was,
#   3. reads what the folder on the host then holds and names what is not part of the page,
#   4. asks the page for its front page, its script and style, its headers, the forward from
#      http and its "not found" page, and says what it found.
#
# Set once, by hand, at the host: the name darkburn.phobic.no for the folder
# httpd.www/darkburn, the way the other names under phobic.no are set up there. Until then
# step 4 says that the page does not answer, and the upload is still fine.
#
# Nothing is removed on the host but the script's own x.new files. A file the page no longer
# uses stays there, and a page that stays there is still shown at its address, until somebody
# deletes it by hand. Step 3 names such files.
#
# For a trial on another host or in another folder:
#   DARKBURN_SITE_HOST   the host
#   DARKBURN_SITE_DIR    the folder; its last part is "darkburn" or starts with "darkburn-"
#   DARKBURN_SITE_URL    the address to ask in step 4; a trial without it is not asked
# And for a host or a day that needs it:
#   DARKBURN_SITE_ANY_STATE=1   upload the real page from a state that is not a clean main
#   DARKBURN_SITE_IN_PLACE=1    write each file straight over the old one. Only for a host
#                               that cannot give a file the place of another: the upload then
#                               stops at the first "rename", with the page as it was.
set -eu

die() {
    printf 'deploy-site: %s\n' "$1" >&2
    exit 1
}

[ $# -eq 1 ] || die "say who uploads:  sh scripts/deploy-site.sh USER   (the SFTP user of the web host)"
user=$1
real_host=ssh.phobic.no
real_dir=httpd.www/darkburn
real_url=https://darkburn.phobic.no
# Under this address the host may show the folder too. .htaccess sends it on to the own name.
real_folder_url=https://phobic.no/darkburn/
# A variable that is set but empty is a mistake, not the real page.
host=${DARKBURN_SITE_HOST-$real_host}
dir=${DARKBURN_SITE_DIR-$real_dir}
if [ "$host" = "$real_host" ] && [ "$dir" = "$real_dir" ]; then
    real=yes
    url=${DARKBURN_SITE_URL-$real_url}
else
    real=no
    url=${DARKBURN_SITE_URL-}
fi
url=${url%/}

# These end up on a command line and in a list of commands for sftp.
case $user in
    '' | -* | *[!A-Za-z0-9._@-]*) die "the user name may hold letters, digits, dot, dash, underscore and @, and does not start with a dash." ;;
esac
case $host in
    '' | -* | *[!A-Za-z0-9.-]*) die "the host name may hold letters, digits, dot and dash, and does not start with a dash." ;;
esac
case $dir in
    '' | -* | /* | */ | *..* | *//* | *[!A-Za-z0-9._/-]*) die "the folder on the host is a plain path from the login folder, such as $real_dir." ;;
esac
# Not the login folder, not the folder that holds all the sites and not the folder of another
# site, by any spelling: the files of this page would take the place of that site's files,
# and its .htaccess would rule every folder below.
case /$dir/ in
    */./*) die "the folder on the host is a plain path from the login folder, such as $real_dir." ;;
esac
case $dir in
    */darkburn | */darkburn-*) ;;
    *) die "the folder on the host is a folder of its own inside the folder of the sites, and its name is darkburn or starts with darkburn-, such as $real_dir or $real_dir-trial: not $dir." ;;
esac
case $url in
    '' | http://?* | https://?*) ;;
    *) die "the address of the page starts with https:// (or http://), such as $real_url." ;;
esac
case $url in
    *[!A-Za-z0-9._~:/@%+-]*) die "the address of the page is a plain address, such as $real_url." ;;
esac

command -v sftp >/dev/null 2>&1 ||
    die "sftp is not installed. It comes with OpenSSH, and with Git for Windows."

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)

# Which state of the repository this is. The page shows the commands people install with, so
# the real page is uploaded from main with nothing uncommitted, and the script says what goes
# up. A trial may come from anywhere.
state="not a git checkout"
if [ -e "$root/.git" ] && command -v git >/dev/null 2>&1 && git -C "$root" rev-parse HEAD >/dev/null 2>&1; then
    state="commit $(git -C "$root" rev-parse --short HEAD) on $(git -C "$root" rev-parse --abbrev-ref HEAD)"
    [ -z "$(git -C "$root" status --porcelain -- site docs/img assets/fonts assets/icons scripts/build-site.sh scripts/deploy-site.sh)" ] ||
        state="$state, with changes that are not committed"
fi
if [ "$real" = yes ] && [ -z "${DARKBURN_SITE_ANY_STATE-}" ]; then
    case $state in
        "commit "*" on main") ;;
        *) die "the real page is uploaded from main with nothing uncommitted. This is: $state. For a trial set DARKBURN_SITE_DIR ($real_dir-trial); to upload this all the same set DARKBURN_SITE_ANY_STATE=1." ;;
    esac
fi

out=$root/target/site
sh "$root/scripts/build-site.sh" "$out"

list=$root/target/site-upload.txt
files=$root/target/site-files.txt
log=$root/target/site-log.txt
seen=$root/target/site-seen.txt
# The lists and what the host answered are removed however the script ends.
trap 'rm -f "$list" "$files" "$log" "$log.there" "$seen" "$seen.page" "$seen.err"' EXIT
trap 'exit 130' INT
trap 'exit 143' HUP TERM

# The files of the page. The pages come last, so a page that is new never points to a picture
# that is not there yet.
{
    (cd "$out" && find . -type f ! -name .made-by-build-site ! -name '*.html' | LC_ALL=C sort)
    (cd "$out" && find . -type f -name '*.html' | LC_ALL=C sort)
} | sed 's|^\./||' >"$files"
count=$(grep -c '' "$files")

# What sftp is to do, one command on a line. A "-" in front: go on if it fails, which mkdir
# does when the folder is there already. Any other command that fails stops the upload.
{
    # The folder of the page and the two in it. Not the folders above it: if they are not
    # there, this is the wrong host or the wrong login, and the first put says so.
    printf -- '-mkdir %s\n-mkdir %s/img\n-mkdir %s/fonts\n' "$dir" "$dir" "$dir"
    printf -- '-chmod 755 %s/img\n-chmod 755 %s/fonts\n' "$dir" "$dir"
    if [ -n "${DARKBURN_SITE_IN_PLACE-}" ]; then
        # A run that stopped at its first "rename" has left every file under its second name.
        while IFS= read -r f; do
            printf 'put %s %s/%s\n-chmod 644 %s/%s\n-rm %s/%s.new\n' "$f" "$dir" "$f" "$dir" "$f" "$dir" "$f"
        done <"$files"
    else
        # Every file goes up under a second name first and is made readable for the web
        # server, whatever its mode is here. The host still shows the old page.
        while IFS= read -r f; do
            printf 'put %s %s/%s.new\n-chmod 644 %s/%s.new\n' "$f" "$dir" "$f" "$dir" "$f"
        done <"$files"
        # Then each takes its place, which is one step on the host.
        while IFS= read -r f; do
            printf 'rename %s/%s.new %s/%s\n' "$dir" "$f" "$dir" "$f"
        done <"$files"
    fi
    # What the three folders hold afterwards.
    printf 'ls -1a %s\nls -1a %s/img\nls -1a %s/fonts\n' "$dir" "$dir" "$dir"
} >"$list"

printf 'Uploading %s files (%s) to %s:%s as %s.\n' "$count" "$state" "$host" "$dir" "$user"
printf 'A line that a folder could not be made ("remote mkdir ...: Failure") only says that it is there already.\n'
[ -z "${DARKBURN_SITE_IN_PLACE-}" ] ||
    printf 'So does a line that a file could not be removed: there was no x.new file left to remove.\n'
# -b stops at the first command that fails. By itself it also forbids the question for a
# password; the option before it allows the question again. What sftp writes about each
# command goes to a file; its questions and its errors still show.
st=0
(cd "$out" && sftp -o BatchMode=no -b "$list" "$user@$host" >"$log") || st=$?
if [ "$st" -ne 0 ]; then
    at=$(grep '^sftp> ' "$log" | tail -n 1 | sed 's/^sftp> //') || true
    if [ -n "${DARKBURN_SITE_IN_PLACE-}" ]; then
        placed=$(grep -c '^sftp> put ' "$log") || true
        how="The host may hold some of the new files and some of the old, and the one that stopped may be cut"
    else
        placed=$(grep -c '^sftp> rename ' "$log") || true
        how="Some files on the host are new and some are old"
    fi
    if [ -z "$at" ]; then
        die "sftp could not log in to $host (see the lines above). Nothing was sent."
    elif [ "$placed" -eq 0 ]; then
        die "the upload stopped at \"$at\" (see the lines above). The page on the host is as it was. Run this again."
    else
        case $at in
            rename*) [ "$placed" -gt 1 ] || die "the upload stopped at \"$at\" (see the lines above): the host could not give a file the place of the old one. The page on the host is as it was. Run this again with DARKBURN_SITE_IN_PLACE=1." ;;
        esac
        die "the upload stopped at \"$at\" (see the lines above). $how. Run this again."
    fi
fi
printf 'Uploaded %s files.\n' "$count"

note() {
    printf 'Look at this: %s\n' "$1"
}

# What the folder on the host holds that is not part of the page. sftp writes the names with
# or without the folder in front, depending on its version; both are read.
awk -v dir="$dir" '
    /^sftp> ls -1a / { cur = substr($0, 14); next }
    /^sftp> / { cur = ""; next }
    cur != "" {
        name = $0
        if (index(name, cur "/") == 1) name = substr(name, length(cur) + 2)
        if (name == "." || name == "..") next
        below = substr(cur, length(dir) + 2)
        print (below == "" ? name : below "/" name)
    }' "$log" | LC_ALL=C sort >"$log.there"
if [ "$(LC_ALL=C sort "$files" | LC_ALL=C comm -23 - "$log.there" | grep -c '')" -ne 0 ]; then
    printf 'The list of the folder on the host could not be read, so look there yourself for files that are not part of the page.\n'
else
    extra=$(LC_ALL=C sort "$files" | LC_ALL=C comm -13 - "$log.there" | grep -vx -e img -e fonts | tr '\n' ' ') || true
    [ -z "$extra" ] ||
        note "the folder on the host also holds: ${extra% }. That is not part of the page. Delete it there by hand unless it is meant to be there: a page that is left is still shown at its address."
fi

# What the host now answers. Only said, never an error: the upload itself went well.
if [ -z "$url" ]; then
    printf 'This was a trial, and DARKBURN_SITE_URL is not set, so the page was not asked.\n'
    exit 0
fi
if ! command -v curl >/dev/null 2>&1; then
    printf 'curl is not installed, so the page was not asked. Look at %s/ yourself.\n' "$url"
    exit 0
fi
# ask ADDRESS: status and where it forwards to, such as "200 " or "301 https://...". curl
# writes 000 when nothing answered; its reason is then in $seen.err.
ask() {
    curl -sS -m 30 -D "$seen" -o "$seen.page" -w '%{http_code} %{redirect_url}' "$1" 2>"$seen.err" || true
}
# said STATUS: the same for a message.
said() {
    case ${1% } in
        000 | '') printf 'nothing' ;;
        *) printf '%s' "${1% }" ;;
    esac
}
got=$(ask "$url/")
case $got in
    '200 ')
        if cmp -s "$seen.page" "$out/index.html"; then
            printf '%s/ answers with the page that was uploaded.\n' "$url"
        else
            note "$url/ answers, but not with the page that was uploaded. The name may point to another folder of the host, or something on the way holds the old page or adds to it."
        fi
        heads=$(tr -d '\r' <"$seen")
        printf '%s\n' "$heads" | grep -qi "^content-security-policy:.*frame-ancestors 'none'" ||
            note "the host does not send the Content-Security-Policy header of .htaccess. It may not read .htaccess files or not allow headers in them, or a rule in the folder above takes its place."
        printf '%s\n' "$heads" | grep -qi '^x-content-type-options:[[:space:]]*nosniff' ||
            note "the host does not send X-Content-Type-Options."
        printf '%s\n' "$heads" | grep -qi '^cache-control:[[:space:]]*no-cache' ||
            note "the host does not send Cache-Control: no-cache, so a browser may show old files after an upload."
        for f in site.js style.css; do
            got=$(ask "$url/$f")
            if [ "$got" != '200 ' ] || ! cmp -s "$seen.page" "$out/$f"; then
                note "$url/$f is not the file that was uploaded (it answers: $(said "$got"))."
            fi
        done
        case $url in
            https://*)
                printf '%s\n' "$heads" | grep -qi '^strict-transport-security:' ||
                    note "the host does not send Strict-Transport-Security."
                plain=http://${url#https://}/
                got=$(ask "$plain")
                case $got in
                    30[1278]" $url/") ;;
                    *) note "$plain does not forward to $url/ (it answers: $(said "$got")). Without https somebody on a visitor's network can change the install commands the page shows." ;;
                esac
                ;;
        esac
        got=$(ask "$url/no-such-page")
        if [ "${got% }" != 404 ]; then
            note "$url/no-such-page should answer 404. It answers: $(said "$got")."
        elif ! cmp -s "$seen.page" "$out/404.html"; then
            note "$url/no-such-page answers 404, but not with the page's own \"not found\" (404.html). The host may not allow ErrorDocument in .htaccess files."
        fi
        if [ "$real" = yes ] && [ "$url" = "$real_url" ]; then
            got=$(ask "$real_folder_url")
            case $got in
                30[1278]" $real_url/" | '404 ' | '403 ' | '410 ') ;;
                *) note "$real_folder_url should forward to $real_url/ or not be there (it answers: $(said "$got")). Under the main name the page shares what a browser keeps apart by name with everything else on phobic.no." ;;
            esac
        fi
        ;;
    30?' '*)
        note "$url/ forwards to ${got#* }, so the page was not asked. If that is where the page is, give that address in DARKBURN_SITE_URL."
        ;;
    '000 ' | '')
        why=$(tr -d '\r' <"$seen.err" | head -n 1)
        case $why in
            *'(60)'* | *'(51)'* | *'(35)'* | *[Cc]ertificate*)
                note "$url/ has no certificate a browser accepts ($why). Give the address to nobody before that is in order, and do not click a browser's warning away."
                ;;
            *)
                note "$url/ does not answer ($why). If the name is new at the host, it can take a while; if it is not set up there, see the top of scripts/deploy-site.sh."
                ;;
        esac
        ;;
    *)
        note "$url/ answers $(said "$got") and not the page. With 403 the host may not be allowed to read the files; with 500 it may not allow a line of .htaccess (its error log says which); else the name may point to another folder of the host."
        ;;
esac
