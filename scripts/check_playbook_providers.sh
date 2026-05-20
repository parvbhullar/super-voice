#!/usr/bin/env bash
# Lint shipped playbooks against the providers registered in
# StreamEngine::default() under the project's default Cargo features
# (carrier, opus, offline). Exits non-zero on the first violation.
#
# Run directly:    bash scripts/check_playbook_providers.sh
# Run via tests:   bash scripts/run_tests.sh playbook_providers
#
# Allowed values are mirrored from src/media/engine.rs. Update both
# places together when registering a new provider.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PLAYBOOK_DIR="${ROOT_DIR}/config/playbook"

# Allowlists (kept in lowercase; comparison is case-sensitive).
ASR_ALLOWED=("sensevoice" "tencent" "aliyun")
TTS_ALLOWED=("supertonic" "aliyun" "tencent" "tencent_basic" "deepgram")
VAD_ALLOWED=("silero" "nop")

# Explicit forbidden set (these tend to creep back via copy-paste).
ASR_FORBIDDEN=("openai" "msedge" "whisper" "faster-whisper" "whisper-ct2" "azure" "cosyvoice")
TTS_FORBIDDEN=("openai" "msedge" "whisper" "azure" "cosyvoice" "silero")

violations=0

contains() {
    local needle="$1"; shift
    local item
    for item in "$@"; do
        [[ "$item" == "$needle" ]] && return 0
    done
    return 1
}

fail() {
    local file="$1" line="$2" field="$3" value="$4" reason="$5"
    printf '\033[31mFAIL\033[0m %s:%s  %s = "%s"\n      %s\n' \
        "${file#${ROOT_DIR}/}" "$line" "$field" "$value" "$reason"
    violations=$((violations + 1))
}

# Extract the YAML frontmatter (between the first two `---` lines).
# Then walk it line-by-line tracking which top-level block we're inside
# (asr / tts / vad) so we can scope `provider:` checks correctly.
check_file() {
    local file="$1"
    local in_frontmatter=0 saw_first_marker=0
    local block="" current_indent=-1
    local lineno=0

    while IFS='' read -r line || [[ -n "$line" ]]; do
        lineno=$((lineno + 1))

        if [[ "$line" == "---" ]]; then
            if (( saw_first_marker == 0 )); then
                saw_first_marker=1
                in_frontmatter=1
                continue
            else
                # Closing frontmatter marker, stop scanning.
                break
            fi
        fi

        (( in_frontmatter == 1 )) || continue
        # Skip pure comment lines and blanks.
        [[ -z "${line//[[:space:]]/}" ]] && continue
        [[ "$line" =~ ^[[:space:]]*# ]] && continue

        # Detect a new top-level block (asr:/tts:/vad:/anything-else).
        if [[ "$line" =~ ^([a-zA-Z_]+):[[:space:]]*$ ]]; then
            block="${BASH_REMATCH[1]}"
            continue
        fi
        if [[ "$line" =~ ^([a-zA-Z_]+):[[:space:]]+ ]]; then
            # Top-level key with inline value — not a nested provider field.
            block="${BASH_REMATCH[1]}"
            continue
        fi

        # We only care about nested provider declarations inside asr/tts/vad.
        case "$block" in
            asr|tts|vad) ;;
            *) continue ;;
        esac

        # Match `<indent>- provider: "value"` (chain entry) OR
        #       `<indent>provider: "value"` (single).
        if [[ "$line" =~ ^[[:space:]]+[-[:space:]]*provider:[[:space:]]*\"?([^\"#[:space:]]+)\"? ]]; then
            local value="${BASH_REMATCH[1]}"
            # Skip env-var indirection: `provider: "${ASR_PROVIDER}"`.
            if [[ "$value" =~ ^\$\{ ]]; then
                continue
            fi
            check_value "$file" "$lineno" "$block.provider" "$value"
            continue
        fi

        # Match chain entries written as `- "value"` directly under
        # `providers:` (we approximate by allowing them in asr/tts blocks).
        if [[ "$line" =~ ^[[:space:]]+-[[:space:]]*\"([^\"#[:space:]]+)\" ]]; then
            local value="${BASH_REMATCH[1]}"
            [[ "$value" =~ ^\$\{ ]] && continue
            check_value "$file" "$lineno" "$block.providers[]" "$value"
            continue
        fi
    done < "$file"
}

check_value() {
    local file="$1" line="$2" field="$3" value="$4"
    local block="${field%%.*}"

    case "$block" in
        asr)
            if contains "$value" "${ASR_FORBIDDEN[@]}"; then
                fail "$file" "$line" "$field" "$value" \
                    "Forbidden ASR provider. Not registered in StreamEngine::default(). Allowed: ${ASR_ALLOWED[*]}."
                return
            fi
            if ! contains "$value" "${ASR_ALLOWED[@]}"; then
                fail "$file" "$line" "$field" "$value" \
                    "Unknown ASR provider. Allowed: ${ASR_ALLOWED[*]}."
            fi
            ;;
        tts)
            if contains "$value" "${TTS_FORBIDDEN[@]}"; then
                fail "$file" "$line" "$field" "$value" \
                    "Forbidden TTS provider. Not registered in StreamEngine::default(). Allowed: ${TTS_ALLOWED[*]}."
                return
            fi
            if ! contains "$value" "${TTS_ALLOWED[@]}"; then
                fail "$file" "$line" "$field" "$value" \
                    "Unknown TTS provider. Allowed: ${TTS_ALLOWED[*]}."
            fi
            ;;
        vad)
            if ! contains "$value" "${VAD_ALLOWED[@]}"; then
                fail "$file" "$line" "$field" "$value" \
                    "Unknown VAD provider. Allowed: ${VAD_ALLOWED[*]}."
            fi
            ;;
    esac
}

main() {
    if [[ ! -d "$PLAYBOOK_DIR" ]]; then
        echo "Playbook directory not found: $PLAYBOOK_DIR" >&2
        exit 2
    fi

    local file
    local scanned=0
    for file in "$PLAYBOOK_DIR"/*.md; do
        # Skip the README — it's documentation, not a runnable playbook.
        [[ "$(basename "$file")" == "README.md" ]] && continue
        # Skip files without a YAML frontmatter (defensive).
        head -1 "$file" | grep -qE '^---' || continue
        check_file "$file"
        scanned=$((scanned + 1))
    done

    if (( violations > 0 )); then
        printf '\n%d violation(s) across %d playbook(s).\n' "$violations" "$scanned" >&2
        exit 1
    fi

    printf 'OK — %d playbook(s) reference only registered providers.\n' "$scanned"
}

main "$@"
