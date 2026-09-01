import { be16, be32, be64, box, concat, fullBox, text } from "./binary.js";

const matrix = concat(
    be32(0x10000), be32(0), be32(0),
    be32(0), be32(0x10000), be32(0),
    be32(0), be32(0), be32(0x40000000),
);

export function makeInitSegment(width, height, timescale, hvcc) {
    const ftyp = box("ftyp", text("iso6"), be32(1), text("iso6"), text("mp41"), text("hvc1"));
    const mvhd = fullBox("mvhd", 0, 0, be32(0), be32(0), be32(timescale), be32(0), be32(0x10000), be16(0x100), be16(0), new Uint8Array(8), matrix, new Uint8Array(24), be32(2));
    const tkhd = fullBox("tkhd", 0, 3, be32(0), be32(0), be32(1), be32(0), be32(0), new Uint8Array(8), be16(0), be16(0), be16(0), be16(0), matrix, be32(width << 16), be32(height << 16));
    const mdhd = fullBox("mdhd", 0, 0, be32(0), be32(0), be32(timescale), be32(0), be16(0x55c4), be16(0));
    const hdlr = fullBox("hdlr", 0, 0, be32(0), text("vide"), new Uint8Array(12), text("VideoHandler\0"));
    const vmhd = fullBox("vmhd", 0, 1, be16(0), be16(0), be16(0), be16(0));
    const dref = fullBox("dref", 0, 0, be32(1), fullBox("url ", 0, 1));
    const visual = concat(
        new Uint8Array(6), be16(1), new Uint8Array(16), be16(width), be16(height),
        be32(0x480000), be32(0x480000), be32(0), be16(1), new Uint8Array(32),
        be16(24), be16(0xffff), box("hvcC", hvcc),
    );
    const stsd = fullBox("stsd", 0, 0, be32(1), box("hvc1", visual));
    const stbl = box(
        "stbl",
        stsd,
        fullBox("stts", 0, 0, be32(0)),
        fullBox("stsc", 0, 0, be32(0)),
        fullBox("stsz", 0, 0, be32(0), be32(0)),
        fullBox("stco", 0, 0, be32(0)),
    );
    const minf = box("minf", vmhd, box("dinf", dref), stbl);
    const trak = box("trak", tkhd, box("mdia", mdhd, hdlr, minf));
    const trex = fullBox("trex", 0, 0, be32(1), be32(1), be32(0), be32(0), be32(0));
    return concat(ftyp, box("moov", mvhd, trak, box("mvex", trex)));
}

export function makeMediaFragment(sequence, timestamp, duration, keyframe, sample) {
    const mfhd = fullBox("mfhd", 0, 0, be32(sequence));
    const tfhd = fullBox("tfhd", 0, 0x020000, be32(1));
    const tfdt = fullBox("tfdt", 1, 0, be64(timestamp));
    const sampleFlags = keyframe ? 0x02000000 : 0x01010000;
    const makeMoof = offset => {
        const trun = fullBox(
            "trun",
            0,
            0x000701,
            be32(1),
            be32(offset),
            be32(duration),
            be32(sample.length),
            be32(sampleFlags),
        );
        return box("moof", mfhd, box("traf", tfhd, tfdt, trun));
    };
    let moof = makeMoof(0);
    moof = makeMoof(moof.length + 8);
    return concat(moof, box("mdat", sample));
}
