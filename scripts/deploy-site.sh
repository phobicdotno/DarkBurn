#!/bin/sh
# Puts the web page, https://darkburn.phobic.no, on its host.
#
#   sh scripts/deploy-site.sh USER
#
# USER is the SFTP user of the web host. sftp asks for the password, or uses your key. The
# password is typed each time: it is in no file here and in no workflow, because the same
# login reaches every other site on that host.
#
# The page is hosted the way the other pages on phobic.no are: a folder of plain files on the
# web host, uploaded over SFTP, with an .htaccess file in it for the headers, the "not found"
# page and https. This script
#   1. builds the folder afresh with scripts/build-site.sh, which also checks it,
#   2. uploads every file in it to httpd.www/darkburn/ on ssh.phobic.no, the pages last,
#   3. asks the page for its front page, its headers, the forward from http and its "not
#      found" page, and says what it found.
#
# Set once, by hand, at the host: the name darkburn.phobic.no for the folder
# httpd.www/darkburn, the way the other names under phobic.no are set up there. Until then
# step 3 says that the page does not answer, and the upload is still fine.
#
# Nothing is removed on the host. A file the page no longer uses stays there until somebody
# deletes it by hand.
#
# Another host, folder or address (for a trial):
#   DARKBURN_SITE_HOST, DARKBURN_SITE_DIR, DARKBURN_SITE_URL
set -eu

die() {
    printf 'deploy-site: %s\n' "$1" >&2
    exit 1
}

[ $# -eq 1 ] || die "say who uploads:  sh scripts/deploy-site.sh USER   (the SFTP user of the web host)"
user=$1
host=${DARKBURN_SITE_HOST:-ssh.phobic.no}
dir=${DARKBURN_SITE_DIR:-httpd.www/darkburn}
url=${DARKBURN_SITE_URL:-https://darkburn.phobic.no}
url=${url%/}

# These three end up on a command line and in a list of commands for sftp.
case $user in
    '' | -* | *[!A-Za-z0-9._@-]*) die "the user name may hold letters, digits, dot, dash, underscore and @, and does not start with a dash." ;;
esac
case $host in
    '' | -* | *[!A-Za-z0-9.-]*) die "the host name may hold letters, digits, dot and dash, and does not start with a dash." ;;
esac
case $dir in
    '' | -* | /* | */ | *..* | *//* | *[!A-Za-z0-9._/-]*) die "the folder on the host is a plain path from the login folder, such as httpd.www/darkburn." ;;
esac

command -v sftp >/dev/null 2>&1 ||
    die "sftp is not installed. It comes with OpenSSH, and with Git for Windows."

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
out=$root/target/site
sh "$root/scripts/build-site.sh" "$out"

# What sftp is to do, one command on a line. A "-" in front: go on if it fails, which mkdir
# does when the folder is there already. A put that fails stops the upload. The pages go
# last, so a page that is new never points to a picture that is not there yet.
list=$root/target/site-upload.txt
{
    # The folder of the page and the two in it. Not the folders above it: if they are not
    # there, this is the wrong host or the wrong login, and the first put says so.
    printf -- '-mkdir %s\n-mkdir %s/img\n-mkdir %s/fonts\n' "$dir" "$dir" "$dir"
    (cd "$out" && find . -type f ! -name .made-by-build-site ! -name '*.html' | LC_ALL=C sort) |
        sed 's|^\./||' | while IFS= read -r f; do
        printf 'put %s %s/%s\n' "$f" "$dir" "$f"
    done
    (cd "$out" && find . -type f -name '*.html' | LC_ALL=C sort) |
        sed 's|^\./||' | while IFS= read -r f; do
        printf 'put %s %s/%s\n' "$f" "$dir" "$f"
    done
} >"$list"
count=$(grep -c '^put ' "$list")

printf 'Uploading %s files to %s:%s as %s.\n' "$count" "$host" "$dir" "$user"
printf 'A line "remote mkdir ...: Failure" only says that the folder is there already.\n'
# -b stops at the first put that fails. By itself it also forbids the question for a password;
# the option before it allows the question again.
if ! (cd "$out" && sftp -o BatchMode=no -b "$list" "$user@$host"); then
    rm -f "$list"
    die "the upload stopped (see the lines above). The host may hold some of the new files and some of the old."
fi
rm -f "$list"
printf 'Uploaded %s files.\n' "$count"

# What the host now answers. Only said, never an error: the upload itself went well.
if ! command -v curl >/dev/null 2>&1; then
    printf 'curl is not installed, so the page was not asked. Look at %s/ yourself.\n' "$url"
    exit 0
fi
seen=$root/target/site-seen.txt
note() {
    printf 'Look at this: %s\n' "$1"
}
if curl -fsS -m 30 -D "$seen" -o "$seen.page" "$url/" 2>/dev/null; then
    if cmp -s "$seen.page" "$out/index.html"; then
        printf '%s/ answers with the page that was uploaded.\n' "$url"
    else
        note "$url/ answers, but not with the page that was uploaded. A cache on the way may hold the old one for a while."
    fi
    tr -d '\r' <"$seen" | grep -qi "^content-security-policy:.*frame-ancestors 'none'" ||
        note "the host does not send the Content-Security-Policy header of .htaccess. It may not read .htaccess files, or not allow headers in them."
    tr -d '\r' <"$seen" | grep -qi '^x-content-type-options:[[:space:]]*nosniff' ||
        note "the host does not send X-Content-Type-Options."
    case $url in
        https://*)
            plain=http://${url#https://}/
            to=$(curl -sS -m 30 -o /dev/null -w '%{http_code} %{redirect_url}' "$plain" 2>/dev/null || true)
            case $to in
                "301 $url/") ;;
                *)
                    # curl writes 000 when nothing answered.
                    case ${to% } in
                        000 | '') to=nothing ;;
                        *) to=${to% } ;;
                    esac
                    note "$plain does not forward to $url/ (it answers: $to). Without https somebody on a visitor's network can change the install commands the page shows."
                    ;;
            esac
            ;;
    esac
    lost=$(curl -sS -m 30 -o "$seen.page" -w '%{http_code}' "$url/no-such-page" 2>/dev/null || true)
    case $lost in
        000 | '') lost=nothing ;;
    esac
    if [ "$lost" != 404 ]; then
        note "$url/no-such-page should answer 404. It answers: $lost."
    elif ! cmp -s "$seen.page" "$out/404.html"; then
        note "$url/no-such-page answers 404, but not with the page's own \"not found\" (404.html). The host may not allow ErrorDocument in .htaccess files."
    fi
else
    note "$url/ does not answer yet. If the name is new at the host, it can take a while; if it is not set up there, see the top of scripts/deploy-site.sh."
fi
rm -f "$seen" "$seen.page"
