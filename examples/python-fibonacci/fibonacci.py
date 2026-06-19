"""Small Fibonacci helpers used by the tiffany-loop example project."""


def fib(n: int) -> int:
    """Return the nth Fibonacci number, where fib(0) == 0."""
    if n < 0:
        raise ValueError("n must be non-negative")
    if n < 2:
        return n

    previous = 0
    current = 1
    for _ in range(2, n + 1):
        previous, current = current, previous + current
    return current


def fib_sequence(count: int) -> list[int]:
    """Return the first count Fibonacci numbers."""
    if count < 0:
        raise ValueError("count must be non-negative")
    return [fib(index) for index in range(count)]
