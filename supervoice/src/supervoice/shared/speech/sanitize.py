from __future__ import annotations

import re

_CODE_BLOCK = re.compile(r"```.*?```", flags=re.DOTALL)
_INLINE_CODE = re.compile(r"`([^`]+)`")
_BOLD = re.compile(r"\*\*([^*]+)\*\*")
_ITALIC = re.compile(r"(?<!\*)\*([^*]+)\*(?!\*)")
_HEADER = re.compile(r"^#{1,6}\s+", flags=re.MULTILINE)
_URL = re.compile(r"https?://\S+")
_MULTI_SPACE = re.compile(r"[ \t]+")
_MULTI_NEWLINE = re.compile(r"\n{3,}")


def sanitize_for_tts(text: str) -> str:
    """Strip markdown, code, URLs from agent text before TTS synthesis."""
    text = _CODE_BLOCK.sub("", text)
    text = _INLINE_CODE.sub(r"\1", text)
    text = _BOLD.sub(r"\1", text)
    text = _ITALIC.sub(r"\1", text)
    text = _HEADER.sub("", text)
    text = _URL.sub("", text)
    text = _MULTI_SPACE.sub(" ", text)
    text = _MULTI_NEWLINE.sub("\n\n", text)
    return text.strip(" \t")
