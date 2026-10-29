#!/bin/sh
# Refuses personal data in git: downloaded manga pages, Studio work (audio, videos, scores),
# app data and databases, local settings, and anything that looks like a key or token.
# Run by .githooks/pre-commit (staged files) and .githooks/pre-push (every file the push adds).
#   scripts/check-private.sh staged
#   scripts/check-private.sh range <from> <to>
# Enable once per clone:  git config core.hooksPath .githooks
set -u

mode=${1:-staged}
if [ "$mode" = range ]; then
  files=$(git diff --name-only --diff-filter=ACMR "$2" "$3" --)
  added() { git diff -U0 --diff-filter=ACMR "$2" "$3" -- "$1" | grep '^+' | grep -v '^+++'; }
else
  files=$(git diff --cached --name-only --diff-filter=ACMR --)
  added() { git diff --cached -U0 --diff-filter=ACMR -- "$1" | grep '^+' | grep -v '^+++'; }
fi

bad=0
say() { echo "  BLOCKED: $1" >&2; bad=1; }

# Files that may be media: the app's own icons and logo.
allowed() {
  case "$1" in
    native/crates/mv-ui/assets/*|"logo manga vox.png") return 0 ;;
  esac
  return 1
}

IFS='
'
for f in $files; do
  lower=$(printf '%s' "$f" | tr 'A-Z' 'a-z')
  case "$lower" in
    *.mangavox/*|*library/manga/*|*studio/projects/*|*/downloads/*|downloads/*)
      say "$f (app data / downloaded manga folder)"; continue ;;
    *settings.local.json|*.env|*.env.*|*/.env|.env|*.pem|*.key|*.p12|*.pfx|*id_rsa*)
      say "$f (local settings or credentials)"; continue ;;
    *.db|*.db-shm|*.db-wal|*.sqlite|*.sqlite3)
      say "$f (database)"; continue ;;
    *.mp4|*.mkv|*.mov|*.webm|*.avi|*.wav|*.mp3|*.m4a|*.flac|*.ogg|*.opus|*.aac|*.srt|*.ass|*.cbz|*.cbr|*.epub|*.pdf)
      allowed "$f" || { say "$f (video / audio / book file)"; continue; } ;;
    *.jpg|*.jpeg|*.png|*.webp|*.gif|*.avif|*.bmp|*.tif|*.tiff)
      allowed "$f" || { say "$f (image: manga pages and panels stay on your PC)"; continue; } ;;
  esac
  if [ "$mode" = range ]; then
    size=$(git cat-file -s "$3:$f" 2>/dev/null || echo 0)
  else
    size=$(git cat-file -s ":$f" 2>/dev/null || echo 0)
  fi
  if [ "$size" -gt 5000000 ]; then
    say "$f (over 5 MB)"; continue
  fi
  if added "$f" | grep -qE -e '-----BEGIN [A-Z ]*PRIVATE KEY-----' -e 'gh[pousr]_[A-Za-z0-9]{30,}' -e 'sk-(ant-)?[A-Za-z0-9_-]{20,}' -e 'AIza[0-9A-Za-z_-]{30,}' -e 'xox[abpr]-[A-Za-z0-9-]{10,}' -e '(api[_-]?key|secret|token|password)["'"'"' ]*[:=] *["'"'"'][A-Za-z0-9_./+=-]{16,}'; then
    say "$f (looks like it contains a key, token or password)"
  fi
done

if [ "$bad" -ne 0 ]; then
  echo "Personal data check failed: the files above stay on this PC. Remove them from the commit (git restore --staged <file>)." >&2
  exit 1
fi
exit 0
