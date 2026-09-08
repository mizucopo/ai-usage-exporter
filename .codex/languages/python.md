# Python guidance

Run the Python quality gate from the repository root:

```bash
uv run task check
```

Apply automatic Ruff fixes separately with `uv run task fix`, then rerun the non-mutating check.
