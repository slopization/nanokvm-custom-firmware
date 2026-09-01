import { be16, be32, concat } from "./binary.js";

export function splitAnnexB(bytes) {
    const starts = [];
    for (let index = 0; index + 3 <= bytes.length;) {
        let prefix = 0;
        if (index + 4 <= bytes.length && bytes[index] === 0 && bytes[index + 1] === 0 && bytes[index + 2] === 0 && bytes[index + 3] === 1) prefix = 4;
        else if (bytes[index] === 0 && bytes[index + 1] === 0 && bytes[index + 2] === 1) prefix = 3;
        if (prefix) {
            starts.push([index, prefix]);
            index += prefix;
        } else index++;
    }
    return starts.map(([start, prefix], position) =>
        bytes.subarray(start + prefix, starts[position + 1]?.[0] ?? bytes.length));
}

export const nalType = nal => nal.length >= 2 ? (nal[0] >> 1) & 0x3f : -1;

function rbsp(nal) {
    const output = [];
    let zeros = 0;
    for (const value of nal.subarray(2)) {
        if (zeros >= 2 && value === 3) {
            zeros = 0;
            continue;
        }
        output.push(value);
        zeros = value === 0 ? zeros + 1 : 0;
    }
    return new Uint8Array(output);
}

class Bits {
    constructor(bytes) { this.bytes = bytes; this.offset = 0; }
    read(count) {
        let value = 0;
        for (let index = 0; index < count; index++) {
            if (this.offset >= this.bytes.length * 8) throw new Error("truncated HEVC SPS");
            value = value * 2 + ((this.bytes[this.offset >> 3] >> (7 - (this.offset & 7))) & 1);
            this.offset++;
        }
        return value;
    }
    ue() {
        let zeros = 0;
        while (this.read(1) === 0) {
            if (++zeros > 31) throw new Error("invalid HEVC Exp-Golomb value");
        }
        return (2 ** zeros - 1) + (zeros ? this.read(zeros) : 0);
    }
}

export function parseHevcSps(sps) {
    if (nalType(sps) !== 33) throw new Error("expected HEVC SPS");
    const bits = new Bits(rbsp(sps));
    bits.read(4);
    const maxSubLayersMinus1 = bits.read(3);
    const temporalIdNested = bits.read(1);
    const profileSpace = bits.read(2);
    const tierFlag = bits.read(1);
    const profileIdc = bits.read(5);
    const compatibility = bits.read(32) >>> 0;
    const constraints = new Uint8Array(6);
    for (let index = 0; index < constraints.length; index++) constraints[index] = bits.read(8);
    const levelIdc = bits.read(8);
    const subProfile = [];
    const subLevel = [];
    for (let index = 0; index < maxSubLayersMinus1; index++) {
        subProfile.push(bits.read(1));
        subLevel.push(bits.read(1));
    }
    if (maxSubLayersMinus1) {
        for (let index = maxSubLayersMinus1; index < 8; index++) bits.read(2);
    }
    for (let index = 0; index < maxSubLayersMinus1; index++) {
        if (subProfile[index]) bits.read(88);
        if (subLevel[index]) bits.read(8);
    }
    bits.ue();
    const chromaFormat = bits.ue();
    if (chromaFormat === 3) bits.read(1);
    const width = bits.ue();
    const height = bits.ue();
    if (bits.read(1)) {
        bits.ue();
        bits.ue();
        bits.ue();
        bits.ue();
    }
    const bitDepthLumaMinus8 = bits.ue();
    const bitDepthChromaMinus8 = bits.ue();
    return { maxSubLayersMinus1, temporalIdNested, profileSpace, tierFlag, profileIdc, compatibility, constraints, levelIdc, chromaFormat, width, height, bitDepthLumaMinus8, bitDepthChromaMinus8 };
}

function reverse32(value) {
    let output = 0;
    for (let bit = 0; bit < 32; bit++) {
        output = (output * 2 + ((value >>> bit) & 1)) >>> 0;
    }
    return output >>> 0;
}

export function hevcCodecString(info) {
    const spaces = ["", "A", "B", "C"];
    let value = `hvc1.${spaces[info.profileSpace]}${info.profileIdc}.${reverse32(info.compatibility).toString(16).toUpperCase()}.${info.tierFlag ? "H" : "L"}${info.levelIdc}`;
    let last = info.constraints.length;
    while (last && info.constraints[last - 1] === 0) last--;
    for (const byte of info.constraints.subarray(0, last)) {
        value += `.${byte.toString(16).toUpperCase().padStart(2, "0")}`;
    }
    return value;
}

export function makeHvcc(vps, sps, pps) {
    const info = parseHevcSps(sps);
    const arrays = [vps, sps, pps].map(nal =>
        concat(new Uint8Array([0x80 | nalType(nal)]), be16(1), be16(nal.length), nal));
    const header = concat(
        new Uint8Array([1, (info.profileSpace << 6) | (info.tierFlag << 5) | info.profileIdc]),
        be32(info.compatibility),
        info.constraints,
        new Uint8Array([info.levelIdc]),
        be16(0xf000),
        new Uint8Array([0xfc, 0xfc | info.chromaFormat, 0xf8 | info.bitDepthLumaMinus8, 0xf8 | info.bitDepthChromaMinus8]),
        be16(0),
        new Uint8Array([(info.maxSubLayersMinus1 + 1) << 3 | info.temporalIdNested << 2 | 3, arrays.length]),
    );
    return { hvcc: concat(header, ...arrays), info, codec: hevcCodecString(info) };
}

export function annexBToSample(bytes) {
    const parts = [];
    for (const nal of splitAnnexB(bytes)) {
        const type = nalType(nal);
        if (type === 32 || type === 33 || type === 34 || type === 35 || type === 38) continue;
        parts.push(be32(nal.length), nal);
    }
    return concat(...parts);
}
