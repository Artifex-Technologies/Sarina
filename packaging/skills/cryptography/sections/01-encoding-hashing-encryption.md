# Encoding, hashing and encryption are three different things

Encoding changes the form of data so it can travel or be stored; anyone can undo it, no key exists. Base64 turns bytes into letters
for e-mail and JSON; hex writes each byte as two digits; URL encoding escapes characters a web address cannot hold; base32, base58
(Bitcoin addresses, no 0/O/I/l) and Ascii85 (PDF) are the same idea. Encoding is not secrecy. Encodings nest: base64 of hex of text is
common in puzzles, and crypto_decode with auto follows the chain.

Hashing turns any input into a fixed-size digest (MD5 128 bits, SHA-1 160, SHA-256 256, SHA-512 512) so that the same input always
gives the same digest and a different input almost surely gives a different one. It is one way: nothing computes the input from the
digest. Hashes check integrity (a download matches the published SHA-256), identify content (Git object ids are SHA-1), and store
passwords (with a salt and a slow function, see Passwords). "What does this hash say" has one honest answer: nothing, unless you have
a candidate input to hash and compare (crypto_hash with expected). Lists of precomputed hashes (rainbow tables, lookup sites) can find
the input of an unsalted hash of a common word; that is a lookup, not a reversal.

Encryption hides data so that only the holder of a key can read it. Symmetric (AES, ChaCha20): one shared key encrypts and decrypts.
Asymmetric (RSA, elliptic curves): a public key encrypts or verifies, the matching private key decrypts or signs. Without the key,
modern encryption cannot be read; "decrypt this AES" with no key is not a task anyone can do.

Telling them apart: a digest has a fixed length of hex digits (32, 40, 64, 128); base64 ends in = or == and decodes to something;
ciphertext looks random (entropy near 8 bits per byte) and decodes to nothing readable; a classical cipher is letters with the wrong
frequencies. crypto_identify applies these tests and shows its evidence.
