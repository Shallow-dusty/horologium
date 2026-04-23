#!/usr/bin/env python3
"""Slim the LiteLLM pricing JSON down to the subset Horologium embeds.

Usage:
    python3 scripts/gen-pricing.py <litellm-full.json> <output.json>

Regeneration workflow:
    curl -sS -o /tmp/litellm.json \\
      https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json
    python3 scripts/gen-pricing.py /tmp/litellm.json data/litellm-anthropic-pricing.json

Filters:
- Only bare `claude-*` keys (drops AWS/Vertex/OpenRouter-routed variants
  like `anthropic.claude-...` or `openrouter/anthropic/...`; those all
  share prices with the bare key and would only bloat the snapshot).
- Only the four cost fields the Rust lookup consumes — other fields like
  `max_tokens` or `mode` are stripped.
- Deterministic sorted key order so git diffs on regeneration stay sane.
"""

import json
import sys
from pathlib import Path

KEEP_FIELDS = (
    "input_cost_per_token",
    "output_cost_per_token",
    "cache_creation_input_token_cost",
    "cache_read_input_token_cost",
)


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__)
        return 2

    src = Path(sys.argv[1])
    dst = Path(sys.argv[2])

    with src.open() as f:
        raw = json.load(f)

    slim = {}
    for key, value in raw.items():
        if not key.startswith("claude-"):
            continue
        if not isinstance(value, dict):
            continue
        trimmed = {f: value[f] for f in KEEP_FIELDS if f in value}
        if trimmed:
            slim[key] = trimmed

    dst.parent.mkdir(parents=True, exist_ok=True)
    with dst.open("w") as f:
        json.dump(slim, f, indent=2, sort_keys=True)
        f.write("\n")

    print(f"wrote {len(slim)} rows to {dst}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
