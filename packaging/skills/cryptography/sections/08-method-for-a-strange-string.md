# Method: "I found this string, what is it?"

1. Look, in code: crypto_identify. It ranks encodings, hashes, tokens, keys and classical ciphers with evidence (character set, length,
   entropy, letter statistics, a decoded preview). Start from its top guess; if two guesses are close, say both.
2. Encoded: crypto_decode from auto. It peels layers (base64 of hex of text) and shows the chain and the text. Decoded bytes that are
   not text (high entropy) are probably compressed or encrypted: say that, and that a key would be needed.
3. A classical cipher: crypto_classic with crack (caesar, vigenere, railfence, substitution). Quote the best guess and its common-word
   score; a low score means the guess is wrong, not that the text is nonsense.
4. A hash: say which algorithms produce that length, that it is one way, and what a candidate comparison would need. If the owner names
   what it might be the hash of, crypto_hash with expected tells them.
5. A token, key or certificate: crypto_modern. Report its claims and expiry and say in the same breath whether the signature was
   verified (it was not unless the key was given).
6. Bytes XORed with a short key: crypto_identify recovers the key and text when there is one.
7. Nothing fits: say so, with the entropy (near 8 bits per byte: encrypted, compressed or random; lower: structured data in a form the
   tools do not know) and the next sensible step (where did it come from, what program made it).

Puzzle (CTF-style) habits that help: a flag often looks like flag{...}; encodings nest; "look at the letters" (Caesar) before "look at
the math"; a file is not always what its extension says (stego_inspect); the answer is wrong until the decoded text reads as English.

Always state: what it is, the evidence, the content, and what cannot be known.
