import unittest

from format import house_join


class TestHouseJoin(unittest.TestCase):
    def test_two(self):
        self.assertEqual(house_join(["a", "b"]), "a :: b")

    def test_three(self):
        self.assertEqual(house_join(["x", "y", "z"]), "x :: y :: z")

    def test_single(self):
        self.assertEqual(house_join(["solo"]), "solo")


if __name__ == "__main__":
    unittest.main()
