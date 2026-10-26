# Hashes, MACs, signatures and key derivation

Hash: a one-way digest (SHA-256, SHA-512, SHA-3, BLAKE2). MD5 and SHA-1 are broken for collisions (two different inputs with the same
digest can be made), so they must not be used for signatures or certificates; they still work as checksums against accidental
corruption. CRC32 is a checksum, not a hash: easy to forge, fine for detecting line noise.

MAC (message authentication code): a hash keyed with a shared secret, HMAC-SHA256 most often. It proves the message came from someone
with the key and was not changed. Verify a MAC with a constant-time comparison (crypto_hash does): comparing byte by byte and stopping
at the first difference leaks, through timing, how many bytes were right.

Digital signature: made with a private key, checked with the public key; proves origin and integrity to anyone, not just a key
holder. RSA-PSS, ECDSA, Ed25519. A signature is only meaningful once verified with a key you trust: decoding a signed token shows its
claims but proves nothing (crypto_modern says NOT VERIFIED until it is given the key). JWTs with alg "none" carry no signature at all
and must be rejected by servers.

Key derivation: a password is not a key. PBKDF2, scrypt, Argon2 stretch a password into a key slowly and with a salt, so guessing is
expensive. HKDF derives several keys from one strong secret (not for passwords).

Checksums for downloads: compare the published SHA-256 with crypto_hash of the file (path, expected); a mismatch means a different
file, not necessarily an attack (a changed release, a broken download), and the publisher's signature (GPG, minisign) is what proves
origin.
