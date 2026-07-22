# Security

## Experimental firmware

Moonlink and the provided NanoKVM image are experimental and have not received
a security audit. Assume that protocol parsing, pairing, device management,
the Web UI, and the image build chain may contain vulnerabilities.

## Unencrypted video

The Moonlight video stream is transmitted as unencrypted UDP packets. Pairing
and encrypted control traffic do not provide video confidentiality. Anyone who
can observe the network path may be able to capture the displayed video.

Audio and control encryption behavior depends on client negotiation and must
not be treated as protection for the video stream.

## Plaintext management UI

The management UI listens on HTTP port `8080`, not HTTPS. It authenticates with
the system root password. The recovery code and root password can therefore be
exposed to a network observer while they are submitted.

## Deployment guidance

- Use a dedicated management VLAN or a direct trusted LAN.
- Do not forward Moonlink ports from an Internet router.
- Do not expose TCP port `8080` outside the management network.
- Apply host and network firewall rules that permit only known clients.
- Treat paired client certificates and `/var/lib/moonlink` as secrets.
- Reflash the device and rotate credentials after suspected compromise.

Security-sensitive production use is not recommended in the current state.
