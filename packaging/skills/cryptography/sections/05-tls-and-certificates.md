# TLS, certificates and keys

TLS (what https uses) does three things: agrees a session key (X25519 or ECDHE, forward secret), proves the server's identity with a
certificate, and then encrypts the traffic with AES-GCM or ChaCha20-Poly1305. TLS 1.2 and 1.3 are current; SSL 3, TLS 1.0 and 1.1 are
retired.

A certificate binds a public key to a name (the subject, with Subject Alternative Names for the host names), is signed by an issuer
(a certificate authority, or itself: self-signed), and is valid between two dates. A chain leads from the server's certificate through
intermediates to a root the computer trusts. crypto_modern reads what a certificate says about itself: subject, issuer, names, dates and
whether it is expired now, key type and size, signature algorithm, fingerprint, whether it is a CA. It does not check the chain against
a root store, so "valid now" means the dates, not trust.

Weak signs: an RSA key under 2048 bits, an SHA-1 or MD5 signature, an expired or not-yet-valid period, a self-signed certificate where
a public one was expected, a name that does not match the host. A self-signed certificate is normal for a home server or a test, and
the browser's warning then means "I cannot vouch for this", not "this is malicious".

Keys on disk: a PEM block that says PRIVATE KEY is a secret: it is never pasted, printed or read into a conversation (the tools refuse);
the matching PUBLIC KEY or certificate can be shared freely. SSH keys: ssh-ed25519 is the current choice; the fingerprint
(SHA256:...) is how a key is recognised; the .pub file is the public half.

Fingerprints: a SHA-256 of the certificate or key, used to recognise it out of band (the one the server shows must match the one you
were given).
