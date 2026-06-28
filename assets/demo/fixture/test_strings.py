import unittest

from strings import slugify


class TestSlugify(unittest.TestCase):
    def test_basic(self):
        self.assertEqual(slugify("Hello World"), "hello-world")

    def test_punctuation(self):
        self.assertEqual(slugify("Foo, Bar!  Baz"), "foo-bar-baz")

    def test_collapse(self):
        self.assertEqual(slugify("a---b   c"), "a-b-c")

    def test_strip(self):
        self.assertEqual(slugify("  -Hi-  "), "hi")


if __name__ == "__main__":
    unittest.main()
