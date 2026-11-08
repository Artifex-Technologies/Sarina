# Classical ciphers and how they are broken

These are puzzles and history, not security; crypto_classic computes them and cracks them.

Caesar (ROT-N): shift every letter by N. 26 possibilities: try them all and score each against English letter frequencies; the right
one has the lowest chi-squared and readable words. ROT13 is the shift 13, its own inverse. Atbash mirrors the alphabet (a-z, b-y).

Affine: each letter x becomes a*x+b (mod 26); a must be coprime with 26 (1 3 5 7 9 11 15 17 19 21 23 25), so there are only 312 keys
and the cracker tries them all, like Caesar (a=1 is a Caesar shift). crypto_classic cipher "affine", key "a,b" (for example "5,8").

Monoalphabetic substitution: each letter replaced by a fixed other letter (26! keys, far too many to try). Broken by frequency
analysis: in English e, t, a, o, i, n are the most common letters, th, he, in, er the most common pairs, "the" the most common word;
single-letter words are a or I. The cracker hill-climbs a key towards those statistics; it needs a few hundred letters of text.

Vigenère: a Caesar shift that changes with each letter of a repeating key word. Each key position is its own Caesar cipher, so: find the
key length (Kasiski: distances between repeated fragments share a factor; Friedman: the index of coincidence of every n-th letter is
English-like, 0.066, at the right n and flat, 0.038, otherwise), then solve each column as a Caesar. The key word is read off the shifts.

Beaufort: like Vigenère, but each letter becomes key letter minus plain letter, so encoding and decoding are the same operation. It falls
the same way (each column of every n-th letter is one alphabet: Kasiski and the index of coincidence find n). crypto_classic cipher
"beaufort", key = the word.

Autokey: Vigenère whose key is a short primer followed by the plaintext itself, so the key never repeats and Kasiski and the
per-column index of coincidence find nothing. It falls anyway: with a primer of n letters, every n-th letter forms a chain that the
one primer letter decides, so each chain is 26 tries scored by letter frequencies. crypto_classic cipher "autokey", key = the primer.

When asked how a cipher was broken, use the "How it fell" lines that every crack answer ends with (letter frequencies against English,
the index of coincidence, the Kasiski distances and their factors, the index of coincidence per key length): that is the work that was
done; do not describe a method the tool did not use.

Transposition (rail fence, columnar): letters are moved, not replaced, so the letter frequencies are still English; the cracker tries
rail counts and scores the words that appear.

Columnar transposition: the text is written in rows as wide as the key word and read off column by column, in the alphabetical order
of the key's letters (no padding: the last row may be short). The cracker tries every column order for key lengths 2 to 8 and scores
common letter pairs (and words, when the text has spaces); it finds the order, written as the rank of each column ("6 3 2 4 1 5"),
not the key word, since any word with letters in that order gives the same cipher. On short text without spaces the message can come
out rotated (a few letters of its end first); the tool says when that can happen. crypto_classic cipher "columnar", key = a word or
the ranks.

XOR with a short key: each byte XORed with a repeating key. A single-byte key is brute-forced (256 tries); a longer key's length is
found by the Hamming distance between blocks, then each position is a single-byte problem (crypto_identify does this when it sees
bytes that are not text). XOR with a key as long as the message and never reused is the one-time pad, the only unbreakable cipher, and
also the one nobody can use at scale.

What they teach: secrecy must rest in the key, not the method (Kerckhoffs); a small key space is no protection; statistics of the
plaintext leak through any cipher that preserves them, which is why modern ciphers make ciphertext look random.
