#!/bin/sh
# Claude Code runs this when a session starts (.claude/settings.json). It turns on the pre-push hook and, in a cloud
# session, installs what the hook's checks need: ALSA's headers, for rts-platform's sound device.
git -C "$CLAUDE_PROJECT_DIR" config core.hooksPath .githooks
if [ "$CLAUDE_CODE_REMOTE" = "true" ] && command -v apt-get >/dev/null 2>&1; then
  missing=""
  for p in libasound2-dev; do dpkg -s "$p" >/dev/null 2>&1 || missing="$missing $p"; done
  if [ -n "$missing" ]; then
    sudo=""; [ "$(id -u)" = 0 ] || sudo="sudo"
    $sudo apt-get install -y -qq $missing >/dev/null 2>&1 \
      || { $sudo apt-get update -qq && $sudo apt-get install -y -qq $missing; } >/dev/null 2>&1 \
      || echo "session-start: could not install$missing; the pre-push checks may fail" >&2
  fi
fi
exit 0
