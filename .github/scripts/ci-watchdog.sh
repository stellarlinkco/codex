#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF' >&2
Usage: ci-watchdog.sh [options] -- <command> [args...]

Options:
  --label <text>                    Human-readable label for logs.
  --heartbeat-seconds <seconds>     Heartbeat interval. Default: 60.
  --wall-timeout-seconds <seconds>  Hard timeout for the wrapped command. Default: 0 (disabled).
  --idle-timeout-seconds <seconds>  Timeout since last log growth. Default: 0 (disabled).
  --log-path <path>                 Path to append captured command output.
  --tail-lines <count>              Number of log lines to print on timeout. Default: 200.
EOF
  exit 2
}

label="command"
heartbeat_seconds=60
wall_timeout_seconds=0
idle_timeout_seconds=0
log_path=""
tail_lines=200

while [[ $# -gt 0 ]]; do
  case "$1" in
    --label)
      [[ $# -ge 2 ]] || usage
      label="$2"
      shift 2
      ;;
    --heartbeat-seconds)
      [[ $# -ge 2 ]] || usage
      heartbeat_seconds="$2"
      shift 2
      ;;
    --wall-timeout-seconds)
      [[ $# -ge 2 ]] || usage
      wall_timeout_seconds="$2"
      shift 2
      ;;
    --idle-timeout-seconds)
      [[ $# -ge 2 ]] || usage
      idle_timeout_seconds="$2"
      shift 2
      ;;
    --log-path)
      [[ $# -ge 2 ]] || usage
      log_path="$2"
      shift 2
      ;;
    --tail-lines)
      [[ $# -ge 2 ]] || usage
      tail_lines="$2"
      shift 2
      ;;
    --)
      shift
      break
      ;;
    *)
      usage
      ;;
  esac
done

[[ $# -gt 0 ]] || usage

if [[ -z "$log_path" ]]; then
  safe_label="${label//[^A-Za-z0-9._-]/_}"
  log_path="$(mktemp "${TMPDIR:-/tmp}/${safe_label}.XXXX.log")"
fi

mkdir -p "$(dirname "$log_path")"
: > "$log_path"

format_duration() {
  local total_seconds="$1"
  local hours=$((total_seconds / 3600))
  local minutes=$(((total_seconds % 3600) / 60))
  local seconds=$((total_seconds % 60))

  if (( hours > 0 )); then
    printf '%dh%02dm%02ds' "$hours" "$minutes" "$seconds"
  elif (( minutes > 0 )); then
    printf '%dm%02ds' "$minutes" "$seconds"
  else
    printf '%ds' "$seconds"
  fi
}

get_log_size() {
  wc -c < "$log_path" 2>/dev/null | tr -d '[:space:]'
}

collect_descendants() {
  local parent_pid="$1"
  local child_pid

  while IFS= read -r child_pid; do
    [[ -n "$child_pid" ]] || continue
    echo "$child_pid"
    collect_descendants "$child_pid"
  done < <(pgrep -P "$parent_pid" 2>/dev/null || true)
}

print_process_tree() {
  local root_pid="$1"
  local process_ids=("$root_pid")
  local child_pid

  while IFS= read -r child_pid; do
    [[ -n "$child_pid" ]] || continue
    process_ids+=("$child_pid")
  done < <(collect_descendants "$root_pid")

  echo "[ci-watchdog] process tree for ${label}:"
  if command -v pstree >/dev/null 2>&1; then
    pstree -ap "$root_pid" || true
  elif command -v ps >/dev/null 2>&1; then
    ps -o pid=,ppid=,pgid=,stat=,etime=,%cpu=,%mem=,command= -p "${process_ids[@]}" 2>/dev/null || \
      echo "[ci-watchdog] ps output unavailable"
  else
    echo "[ci-watchdog] pstree/ps unavailable"
  fi
}

terminate_process_tree() {
  local root_pid="$1"
  local process_ids=("$root_pid")
  local child_pid

  while IFS= read -r child_pid; do
    [[ -n "$child_pid" ]] || continue
    process_ids+=("$child_pid")
  done < <(collect_descendants "$root_pid")

  kill -TERM "${process_ids[@]}" 2>/dev/null || true
  sleep 5
  kill -KILL "${process_ids[@]}" 2>/dev/null || true
}

start_epoch="$(date +%s)"
last_output_epoch="$start_epoch"
last_log_size="$(get_log_size || echo 0)"
next_heartbeat_epoch=$((start_epoch + heartbeat_seconds))
watchdog_reason=""
stream_dir="$(mktemp -d "${TMPDIR:-/tmp}/ci-watchdog-streams.XXXXXX")"
stdout_pipe="${stream_dir}/stdout.pipe"
stderr_pipe="${stream_dir}/stderr.pipe"
poll_interval_seconds=5

for candidate in "$heartbeat_seconds" "$wall_timeout_seconds" "$idle_timeout_seconds"; do
  if (( candidate > 0 && candidate < poll_interval_seconds )); then
    poll_interval_seconds="$candidate"
  fi
done
if (( poll_interval_seconds < 1 )); then
  poll_interval_seconds=1
fi

cleanup_streaming_resources() {
  if [[ -n "${stdout_tee_pid:-}" ]]; then
    wait "$stdout_tee_pid" 2>/dev/null || true
  fi
  if [[ -n "${stderr_tee_pid:-}" ]]; then
    wait "$stderr_tee_pid" 2>/dev/null || true
  fi
  rm -rf "$stream_dir"
}

mkfifo "$stdout_pipe" "$stderr_pipe"
tee -a "$log_path" < "$stdout_pipe" &
stdout_tee_pid=$!
tee -a "$log_path" >&2 < "$stderr_pipe" &
stderr_tee_pid=$!

"$@" > "$stdout_pipe" 2> "$stderr_pipe" &
child_pid=$!

while kill -0 "$child_pid" 2>/dev/null; do
  sleep "$poll_interval_seconds"

  now_epoch="$(date +%s)"
  current_log_size="$(get_log_size || echo "$last_log_size")"
  if [[ "$current_log_size" != "$last_log_size" ]]; then
    last_output_epoch="$now_epoch"
    last_log_size="$current_log_size"
  fi

  elapsed_seconds=$((now_epoch - start_epoch))
  idle_seconds=$((now_epoch - last_output_epoch))

  if (( now_epoch >= next_heartbeat_epoch )); then
    echo "[ci-watchdog] label=${label} elapsed=$(format_duration "$elapsed_seconds") idle=$(format_duration "$idle_seconds") log_bytes=${current_log_size}"
    next_heartbeat_epoch=$((now_epoch + heartbeat_seconds))
  fi

  if (( wall_timeout_seconds > 0 && elapsed_seconds >= wall_timeout_seconds )); then
    watchdog_reason="wall timeout after $(format_duration "$elapsed_seconds")"
    break
  fi

  if (( idle_timeout_seconds > 0 && idle_seconds >= idle_timeout_seconds )); then
    watchdog_reason="idle timeout after $(format_duration "$idle_seconds") without log growth"
    break
  fi
done

if [[ -n "$watchdog_reason" ]]; then
  echo "[ci-watchdog] ${label} exceeded ${watchdog_reason}"
  print_process_tree "$child_pid"
  if [[ -s "$log_path" ]]; then
    echo "[ci-watchdog] tail -n ${tail_lines} ${log_path}:"
    tail -n "$tail_lines" "$log_path" || true
  else
    echo "[ci-watchdog] no captured output in ${log_path}"
  fi
  terminate_process_tree "$child_pid"
  wait "$child_pid" || true
  cleanup_streaming_resources
  exit 124
fi

set +e
wait "$child_pid"
exit_code=$?
set -e
cleanup_streaming_resources
end_epoch="$(date +%s)"
echo "[ci-watchdog] label=${label} completed exit_code=${exit_code} elapsed=$(format_duration "$((end_epoch - start_epoch))") log_path=${log_path}"
exit "$exit_code"
