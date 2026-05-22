from supervoice.shared.speech.sanitize import sanitize_for_tts


def test_strips_markdown_bold_and_italic():
    assert sanitize_for_tts("Hello **world** and *friend*") == (
        "Hello world and friend"
    )


def test_strips_inline_code():
    assert sanitize_for_tts("Run `git status` now") == "Run git status now"


def test_strips_code_blocks():
    text = "Use this:\n```python\nprint('hi')\n```\nDone."
    assert sanitize_for_tts(text) == "Use this:\n\nDone."


def test_strips_urls():
    assert sanitize_for_tts("See https://example.com/foo for details") == (
        "See for details"
    )


def test_strips_markdown_headers():
    assert sanitize_for_tts("# Title\nbody") == "Title\nbody"


def test_collapses_whitespace():
    assert sanitize_for_tts("hello   world\n\n\nfoo") == "hello world\n\nfoo"
