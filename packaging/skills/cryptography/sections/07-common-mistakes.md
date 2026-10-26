# Common mistakes to recognise

- ECB mode: identical blocks in, identical blocks out; patterns survive. Use GCM or ChaCha20-Poly1305.
- Reusing an IV or nonce with CTR, GCM or a stream cipher: the two messages' XOR leaks. A nonce is used once.
- Encryption without authentication (CBC alone): ciphertext can be altered; padding-oracle attacks decrypt it byte by byte by watching
  error messages. Authenticated encryption, and one generic error, close this.
- MD5 or SHA-1 for signatures, certificates or deduplication where an attacker chooses the input: collisions can be made.
- Fast, unsalted password hashes; see Passwords.
- Comparing secrets with a plain string compare: timing leaks; use a constant-time compare.
- Rolling your own cipher, or "encrypting" with base64, XOR with a word, or a Caesar shift.
- Hard-coded keys and tokens in code, config, or git history; private keys in a chat.
- Random numbers from a non-cryptographic generator (rand with a time seed) for keys, tokens or nonces.
- Trusting a decoded JWT or a certificate's dates as proof: only a verified signature and a trusted chain prove anything.
- Confusing encoding with encryption and hashing with encryption (see the first section).
- Accepting alg "none" or letting the token choose the algorithm (a server that verifies RS256 tokens with HS256 and the public key as
  the secret is forged).

When one of these turns up in the owner's own code, say which line and which rule, and the standard fix; do not write an exploit.
