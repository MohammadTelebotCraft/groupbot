"""Tokenizer parity checks for the image-filter evaluator."""

import pathlib
import tempfile
import unittest

from tools.evaluate_imgfilter import Bpe


class BpeTests(unittest.TestCase):
    def test_unicode_line_separator_does_not_shift_ids(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            vocab = root / "vocab.txt"
            merges = root / "merges.txt"
            vocab.write_text("<pad>\n<eos>\n<unk>\n▁\u2028\nz\n", encoding="utf-8")
            merges.write_text("", encoding="utf-8")
            self.assertEqual(Bpe(vocab, merges).encode("z")[0, :2].tolist(), [4, 1])

    def test_cached_export_matches_rust_goldens(self):
        root = pathlib.Path(__file__).resolve().parent.parent / "target" / "release"
        vocab = root / "vision_text_vocab.txt"
        merges = root / "vision_text_merges.txt"
        if not vocab.exists() or not merges.exists():
            self.skipTest("exported tokenizer files are unavailable")
        bpe = Bpe(vocab, merges)
        for phrase, expected in [
            ("a photo of a cat", [194661, 2567, 544, 444, 4192, 1]),
            ("سیگار", [9395, 103211, 1]),
            ("عکس سگ", [40040, 149326, 1]),
        ]:
            self.assertEqual(bpe.encode(phrase)[0, : len(expected)].tolist(), expected)


if __name__ == "__main__":
    unittest.main()
