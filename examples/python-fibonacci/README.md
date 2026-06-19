# Python Fibonacci Example

This is a tiny zero-dependency project for testing tiffany-loop against a real
working directory.

## Run The Tests

```bash
python3 -m unittest discover -s tests
```

## Try tiffany-loop

From this directory:

```bash
tiffany orchestrator
```

Then ask:

```text
Add a lucas(n) function to fibonacci.py, add unit tests, and run python3 -m unittest discover -s tests.
```

For a non-interactive run:

```bash
orchestrator run "Add a lucas(n) function to fibonacci.py, add unit tests, and run python3 -m unittest discover -s tests."
```

The project includes `CLAUDE.md` so Claude Code-style workers have local rules
to follow.
