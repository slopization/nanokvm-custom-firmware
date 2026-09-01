export const be16 = value => new Uint8Array([(value >>> 8) & 255, value & 255]);
export const be24 = value => new Uint8Array([(value >>> 16) & 255, (value >>> 8) & 255, value & 255]);
export const be32 = value => new Uint8Array([(value >>> 24) & 255, (value >>> 16) & 255, (value >>> 8) & 255, value & 255]);
export const be64 = value => concat(be32(Math.floor(value / 0x100000000)), be32(value >>> 0));
export const text = value => new TextEncoder().encode(value);

export function concat(...parts) {
    const size = parts.reduce((total, part) => total + part.length, 0);
    const output = new Uint8Array(size);
    let offset = 0;
    for (const part of parts) {
        output.set(part, offset);
        offset += part.length;
    }
    return output;
}

export function box(type, ...payload) {
    const body = concat(...payload);
    return concat(be32(body.length + 8), text(type), body);
}

export const fullBox = (type, version, flags, ...payload) =>
    box(type, new Uint8Array([version]), be24(flags), ...payload);
