#!/usr/bin/env bash
# agentfx with `render` offloaded to another machine (default: lore, then the softwer overflow box).
# Drop-in: put this first on PATH as `agentfx` (e.g. via AFX_LOCAL_BIN). Every other subcommand,
# and any render when the remote is busy or unreachable, runs locally.
#
#   AFX_REMOTES="lore|0-3|4 afxrender@89.167.79.118|1|0.7"
#                              hosts in order of preference: ssh host|cores (taskset)|max 1-min load.
#                              The first host under its load limit that has ffmpeg (on PATH or
#                              <runner>/bin/ffmpeg) takes the job; none: render locally.
#   AFX_REMOTE_DIR=agentfx-runner   runner dir on the host (bin/, share/); jobs go to /tmp/agentfx-jobs-$USER
#   AFX_LOCAL_BIN=<path>       the local agentfx binary
#
# The working directory (files <= 50 MB, no .git/target/node_modules) is synced to a throwaway job
# dir on the host, the render runs there with nice 10 and the output comes back. Cleanup removes
# exactly the files that were synced plus the output, one by one, then the empty dirs (no
# recursive delete on remote hosts); anything else the renderer left expires with /tmp.
set -euo pipefail
REMOTES=${AFX_REMOTES:-"lore|0-3|4 afxrender@89.167.79.118|1|0.7"}
RDIR=${AFX_REMOTE_DIR:-agentfx-runner}
LOCAL=${AFX_LOCAL_BIN:-$(dirname "$(readlink -f "$0")")/../target/release/agentfx}

local_run() { exec "$LOCAL" "$@"; }

[ "${1:-}" = render ] || local_run "$@"

# find -o/--output; everything else passes through unchanged
args=("$@"); out=""; rargs=()
for ((i = 0; i < ${#args[@]}; i++)); do
  a=${args[i]}
  case "$a" in
    -o|--output) out=${args[i+1]:-}; rargs+=("$a" "__AFX_OUT__"); i=$((i+1)) ;;
    --output=*) out=${a#--output=}; rargs+=("--output=__AFX_OUT__") ;;
    *) rargs+=("$a") ;;
  esac
done
[ -n "$out" ] || local_run "$@"
# absolute paths in the args (other than -o) would not exist remotely
for a in "${rargs[@]}"; do case "$a" in /*) local_run "$@" ;; esac; done

REMOTE=""
for spec in $REMOTES; do
  IFS='|' read -r h c m <<<"$spec"
  probe="echo \"\$(cut -d' ' -f1 /proc/loadavg) \$(id -un) \$( { command -v ffmpeg >/dev/null || test -x ~/$RDIR/bin/ffmpeg; } && echo ff || echo noff)\""
  read -r load ruser ff < <(timeout 6 ssh -o BatchMode=yes -o ConnectTimeout=4 "$h" "$probe" 2>/dev/null || echo "999 x noff")
  if [ "$ff" = ff ] && awk -v l="$load" -v m="$m" 'BEGIN{exit !(l <= m)}'; then REMOTE=$h; CPUS=$c; break; fi
  echo "remote-render: skip $h (load $load, ${ff})" >&2
done
[ -n "$REMOTE" ] || { echo "remote-render: no remote free, rendering locally" >&2; local_run "$@"; }

ext=${out##*.}; [ "$ext" != "$out" ] || ext=mp4
job="job-$(hostname)-$$-$(date +%s)"
rout="__out.$ext"
q=(); for a in "${rargs[@]}"; do q+=("$(printf %q "${a//__AFX_OUT__/$rout}")"); done
echo "remote-render: $REMOTE (load $load) job $job" >&2
JD="/tmp/agentfx-jobs-$ruser/$job"
list=$(mktemp)
cleanup() {
  { cat "$list"; echo "$rout"; } | ssh -o BatchMode=yes "$REMOTE" "cd $JD 2>/dev/null || exit 0
    while IFS= read -r f; do [ -n \"\$f\" ] && [ ! -d \"\$f\" ] && rm -f -- \"\$f\"; done
    find . -depth -mindepth 1 -type d -empty -exec rmdir {} + 2>/dev/null; cd / && rmdir $JD 2>/dev/null; true" >/dev/null 2>&1 || true
  rm -f "$list"
}
trap cleanup EXIT
ssh -o BatchMode=yes "$REMOTE" "mkdir -p $JD"
rsync -a --max-size=50m --exclude .git --exclude target --exclude node_modules --exclude '*.mp4' \
  --out-format='%n' ./ "$REMOTE:$JD/" | grep -v '/$' > "$list" || true
set +e
ssh -o BatchMode=yes "$REMOTE" "cd $JD && PATH=\$HOME/$RDIR/bin:\$PATH AGENTFX_HOME=\$HOME/$RDIR/share AGENTFX_GPU=0 nice -n 10 taskset -c $CPUS \$HOME/$RDIR/bin/agentfx ${q[*]}"
code=$?
set -e
if [ $code -eq 0 ]; then
  mkdir -p "$(dirname "$out")"
  rsync -a "$REMOTE:$JD/$rout" "$out"
fi
exit $code
