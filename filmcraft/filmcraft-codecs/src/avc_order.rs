//! Presentation order of H.264 pictures from their order counts.
//!
//! Modified (filmcraft/PATCHES.md): this function is `mxf.rs`'s, moved here unchanged so that the AVI reader can
//! reorder B-frames in a build without the `mxf` feature (it uses `filmcraft-h264` and `filmcraft-bitstream`, never
//! `filmcraft-mxf`).

/// Picture order counts of AVC access units (Annex B), in stored order, as presentation
/// positions: the rank of (IDR period, POC) (8.2.1.1, picture order count type 0; types 1 and 2
/// never reorder for the streams written by AVC encoders we have seen, so they keep stored order).
pub fn avc_presentation_order(heads: &[Vec<u8>]) -> Option<Vec<i64>> {
    use filmcraft_bitstream::{annexb_nals, unescape_rbsp};
    use filmcraft_h264::params::{Pps, Sps};
    use filmcraft_h264::slice::{NalHeader, SliceHeader};
    let mut spss: Vec<Option<Sps>> = vec![None; 32];
    let mut ppss: Vec<Option<Pps>> = vec![None; 256];
    let (mut prev_msb, mut prev_lsb) = (0i64, 0i64);
    let mut period = 0i64;
    let mut keys: Vec<(i64, i64)> = Vec::with_capacity(heads.len());
    for (i, h) in heads.iter().enumerate() {
        let mut key = None;
        for nal in annexb_nals(h) {
            let Ok(hdr) = NalHeader::parse(nal[0]) else {
                continue;
            };
            let rbsp = unescape_rbsp(&nal[1..]);
            match hdr.nal_unit_type {
                7 => {
                    let sps = Sps::parse(&rbsp).ok()?;
                    let id = sps.id as usize;
                    *spss.get_mut(id)? = Some(sps);
                }
                8 => {
                    let pps = Pps::parse(&rbsp, &spss).ok()?;
                    let id = pps.id as usize;
                    *ppss.get_mut(id)? = Some(pps);
                }
                1 | 5 => {
                    let mut sps_of = None;
                    let (sh, _, _) = SliceHeader::parse(&rbsp, hdr, |id| {
                        let pps = ppss
                            .get(id as usize)
                            .and_then(Option::as_ref)
                            .ok_or(filmcraft_h264::Error::MissingParameterSet("PPS".into()))?;
                        let sps = spss
                            .get(pps.sps_id as usize)
                            .and_then(Option::as_ref)
                            .ok_or(filmcraft_h264::Error::MissingParameterSet("SPS".into()))?;
                        sps_of = Some(sps);
                        Ok((pps, sps))
                    })
                    .ok()?;
                    let sps = sps_of?;
                    if sh.idr {
                        if i > 0 {
                            period += 1;
                        }
                        prev_msb = 0;
                        prev_lsb = 0;
                    }
                    let poc = match sps.pic_order_cnt_type {
                        0 => {
                            let max = 1i64 << sps.log2_max_poc_lsb;
                            let lsb = sh.pic_order_cnt_lsb as i64;
                            let msb = if lsb < prev_lsb && prev_lsb - lsb >= max / 2 {
                                prev_msb + max
                            } else if lsb > prev_lsb && lsb - prev_lsb > max / 2 {
                                prev_msb - max
                            } else {
                                prev_msb
                            };
                            let top = msb + lsb;
                            let poc = if sh.field_pic {
                                top
                            } else {
                                top.min(top + sh.delta_pic_order_cnt_bottom as i64)
                            };
                            if hdr.nal_ref_idc != 0 {
                                prev_msb = msb;
                                prev_lsb = lsb;
                            }
                            poc
                        }
                        _ => i as i64,
                    };
                    if sh.has_mmco5() {
                        // memory_management_control_operation 5: later pictures start a new period
                        period += 1;
                        prev_msb = 0;
                        prev_lsb = 0;
                        key = Some((period - 1, poc));
                    } else {
                        key = Some((period, poc));
                    }
                    break;
                }
                _ => {}
            }
        }
        keys.push(key?);
    }
    let mut order: Vec<usize> = (0..keys.len()).collect();
    order.sort_by_key(|&i| (keys[i], i));
    let mut pts = vec![0i64; keys.len()];
    for (rank, &i) in order.iter().enumerate() {
        pts[i] = rank as i64;
    }
    Some(pts)
}
