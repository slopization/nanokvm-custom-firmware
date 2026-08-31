# Security

## Experimental firmware

Moonlink and the provided NanoKVM image are experimental and have not received
a security audit. Assume that protocol parsing, pairing, device management,
the Web UI, and the image build chain may contain vulnerabilities.

## Stream encryption

Moonlink supports Moonlight's authenticated video encryption and audio
encryption. The global stream encryption policy can be set to `never`,
`opportunistic`, or `mandatory` in the management UI or with
`--encryption-mode`. The default is `opportunistic`, which encrypts media when
the client requests it but permits a plaintext stream otherwise.

Use `mandatory` when confidentiality is required. It requests encrypted audio
and video and rejects clients that do not negotiate both. Control traffic uses
the pairing-derived encryption key independently of the media policy.

## Plaintext management UI

The management UI listens on HTTP port `8080`, not HTTPS. It authenticates with
the system root password. The recovery code and root password can therefore be
exposed to a network observer while they are submitted.

## Deployment guidance

- Use a dedicated management VLAN or a direct trusted LAN.
- Do not forward Moonlink ports from an Internet router.
- Do not expose TCP port `8080` outside the management network.
- Set stream encryption to `mandatory` on networks where traffic can be
  observed by untrusted systems.
- Apply host and network firewall rules that permit only known clients.
- Treat paired client certificates and `/var/lib/moonlink` as secrets.
- Reflash the device and rotate credentials after suspected compromise.

Security-sensitive production use is not recommended in the current state.
