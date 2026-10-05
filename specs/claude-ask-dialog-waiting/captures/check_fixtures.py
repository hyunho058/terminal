#!/usr/bin/env python3
"""R1.4 check: every REAL_* fixture made from these captures is byte-equal to its file.

Decodes each Rust string literal in crates/app/src/prompt.rs (the escapes the generator
emits, plus Rust's line-continuation rule) and compares it with the capture it names.
Run from the repo root: `python3 specs/claude-ask-dialog-waiting/captures/check_fixtures.py`.
Exits non-zero on any mismatch or missing fixture.
"""
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
PROMPT = os.path.join(HERE, "..", "..", "..", "crates", "app", "src", "prompt.rs")

FIXTURES = {
    "REAL_ASK_SINGLE": "c1_ask_single",
    "REAL_ASK_MULTI": "c2_ask_multi",
    "REAL_ASK_REVIEW": "c3_ask_review",
    "REAL_ASK_MULTISELECT": "c4_ask_multiselect",
    "REAL_ASK_FREETEXT": "c5_ask_freetext_typed",
    "REAL_ASK_TALL": "c6_ask_tall",
    "REAL_PLAN_WIDE": "c7_plan_88",
    "REAL_PLAN_ANSWERED": "c7_plan_88_after_esc",
    "REAL_PLAN_NARROW": "c8_plan_57",
    "REAL_PLAN_MIN": "c8_plan_47",
    "REAL_PLAN_TOO_NARROW": "c8_plan_46_resized",
    "REAL_MODEL_PICKER": "c9_model_picker",
    "REAL_RESUME_PICKER": "c9_resume_picker",
    "REAL_ASK_ANSWERED": "c10_after_answer",
    "REAL_ASK_NARROW": "c11_ask_multi_57",
    "REAL_TRUST_DIALOG": "x_workspace_trust_dialog",
}


def literal_after(src, name):
    """The raw source text of the string literal assigned to `const name: &str`."""
    m = re.search(r"const " + name + r': &str = "', src)
    if not m:
        return None
    i, out = m.end(), []
    while True:
        c = src[i]
        if c == "\\":
            out.append(src[i : i + 2])
            i += 2
        elif c == '"':
            return "".join(out)
        else:
            out.append(c)
            i += 1


def decode(raw):
    """Rust string-literal semantics for the escapes that occur here."""
    out, i = [], 0
    while i < len(raw):
        c = raw[i]
        if c != "\\":
            out.append(c)
            i += 1
            continue
        n = raw[i + 1]
        if n == "\n":  # line continuation: skip the newline and the next line's indent
            i += 2
            while i < len(raw) and raw[i] in " \t\n\r":
                i += 1
        elif n == "x":
            out.append(chr(int(raw[i + 2 : i + 4], 16)))
            i += 4
        elif n == "u":
            j = raw.index("}", i)
            out.append(chr(int(raw[i + 3 : j], 16)))
            i = j + 1
        else:
            out.append({"n": "\n", "t": "\t", "r": "\r", "0": "\0", "\\": "\\", '"': '"', "'": "'"}[n])
            i += 2
    return "".join(out)


def main():
    src = open(PROMPT, encoding="utf-8").read()
    bad = 0
    for name, cap in FIXTURES.items():
        raw = literal_after(src, name)
        want = open(os.path.join(HERE, cap + ".txt"), encoding="utf-8").read()
        if raw is None:
            print(f"MISSING {name}")
            bad += 1
        elif decode(raw) != want:
            print(f"DIFFERS {name} vs {cap}.txt")
            bad += 1
    ok = len(FIXTURES) - bad
    print(f"fixtures byte-equal: {ok}/{len(FIXTURES)}")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
