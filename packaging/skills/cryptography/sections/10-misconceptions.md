# Misconceptions to correct gently

- "Base64 is encryption." It is encoding; anyone decodes it. Same for hex, ROT13, a Caesar shift.
- "Hashing is encryption." A hash has no key and no way back; encryption has both.
- "MD5 is cracked, so a file with an MD5 checksum is unsafe." Collisions let an attacker craft two files with the same MD5; a checksum
  still catches a corrupted download. Prefer SHA-256, and a signature for origin.
- "A longer password hash is stronger." The hash length says nothing; the function (bcrypt, Argon2) and the salt do.
- "Decoding the JWT showed the user id, so the token is valid." Decoding shows claims; only verification shows validity.
- "https means the site is honest." It means the traffic is private and the server holds a certificate for that name; a phishing site
  can have one too.
- "Quantum computers break all encryption." They break RSA and elliptic-curve key exchange in principle (Shor), if large enough ones
  are built; symmetric ciphers and hashes keep most of their strength (Grover halves the bits), and post-quantum key exchange (ML-KEM)
  is already deployed in TLS.
- "Encryption is only for people with something to hide." It is what keeps a password out of a coffee-shop Wi-Fi, a photo out of a
  lost phone, a bank session out of a hotel router.
- "The VPN encrypts everything." It encrypts the path to the VPN server; after that, traffic is as private as the site's own TLS.
- "A self-signed certificate is dangerous." It is unvouched-for, which is fine for your own server and wrong for a bank.
