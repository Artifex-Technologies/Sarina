# Classical ciphers and how they are broken

These are puzzles and history, not security; crypto_classic computes them and cracks them.

Caesar (ROT-N): shift every letter by N. 26 possibilities: try them all and score each against English letter frequencies; the right
one has the lowest chi-squared and readable words. ROT13 is the shift 13, its own inverse. Atbash mirrors the alphabet (a-z, b-y).

Monoalphabetic substitution: each letter replaced by a fixed other letter (26! keys, far too many to try). Broken by frequency
analysis: in English e, t, a, o, i, n are the most common letters, th, he, in, er the most common pairs, "the" the most common word;
single-letter words are a or I. The cracker hill-climbs a key towards those statistics; it needs a few hundred letters of text.

Vigenère: a Caesar shift that changes with each letter of a repeating key word. Each key position is its own Caesar cipher, so: find the
key length (Kasiski: distances between repeated fragments share a factor; Friedman: the index of coincidence of every n-th letter is
English-like, 0.066, at the right n and flat, 0.038, otherwise), then solve each column as a Caesar. The key word is read off the shifts.

Transposition (rail fence, columnar): letters are moved, not replaced, so the letter frequencies are still English; the cracker tries
rail counts and scores the words that appear.

XOR with a short key: each byte XORed with a repeating key. A single-byte key is brute-forced (256 tries); a longer key's length is
found by the Hamming distance between blocks, then each position is a single-byte problem (crypto_identify does this when it sees
bytes that are not text). XOR with a key as long as the message and never reused is the one-time pad, the only unbreakable cipher, and
also the one nobody can use at scale.

What they teach: secrecy must rest in the key, not the method (Kerckhoffs); a small key space is no protection; statistics of the
plaintext leak through any cipher that preserves them, which is why modern ciphers make ciphertext look random.
