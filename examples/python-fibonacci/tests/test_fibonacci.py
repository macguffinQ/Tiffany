import unittest

from fibonacci import fib, fib_sequence


class FibonacciTests(unittest.TestCase):
    def test_fib_base_cases(self) -> None:
        self.assertEqual(fib(0), 0)
        self.assertEqual(fib(1), 1)

    def test_fib_known_values(self) -> None:
        self.assertEqual(fib(2), 1)
        self.assertEqual(fib(7), 13)
        self.assertEqual(fib(10), 55)

    def test_fib_rejects_negative_input(self) -> None:
        with self.assertRaises(ValueError):
            fib(-1)

    def test_fib_sequence(self) -> None:
        self.assertEqual(fib_sequence(0), [])
        self.assertEqual(fib_sequence(7), [0, 1, 1, 2, 3, 5, 8])


if __name__ == "__main__":
    unittest.main()
