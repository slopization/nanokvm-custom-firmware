#!/bin/sh
set -eu

if ! command -v modprobe >/dev/null 2>&1; then
    echo "modprobe is not available" >&2
    exit 1
fi

if ! modprobe vivid 2>/tmp/moonlink-vivid.err; then
    echo "failed to load vivid; kernel may not ship media test drivers" >&2
    cat /tmp/moonlink-vivid.err >&2
    exit 1
fi

if ! modprobe vicodec 2>/tmp/moonlink-vicodec.err; then
    echo "failed to load vicodec; kernel may not ship media test drivers" >&2
    cat /tmp/moonlink-vicodec.err >&2
    exit 1
fi

echo "loaded vivid and vicodec"
echo
echo "video devices:"
for node in /dev/video*; do
    [ -e "$node" ] || continue
    if command -v v4l2-ctl >/dev/null 2>&1; then
        echo "== $node =="
        v4l2-ctl -d "$node" --info | sed -n '1,12p'
    else
        echo "$node"
    fi
done

cat <<'EOF'

vicodec produces FWHT, not H.264 or HEVC. Use it with the v4l2-smoke binary
to validate V4L2 queueing, DMABUF export/import, and async dequeue behavior.
It cannot be streamed to Moonlight directly.
EOF
