//! AOM AV1 RTP v1.0.0, single spatial layer. Rebuild the final size-less OBU exactly
//! as DAVE expects; only authenticated plaintext is inspected or normalized for decode.
use super::video_receive::MAX_FRAME_BYTES;

pub(super) fn leb(data: &[u8]) -> Option<(usize, usize)> {
	let mut value = 0u32;
	for (i, &byte) in data.iter().take(8).enumerate() {
		let low = u32::from(byte & 127);
		if i >= 4 && low != 0 {
			return None;
		}
		if i < 4 {
			value |= low << (i * 7);
		}
		if byte & 128 == 0 {
			return Some((value as usize, i + 1));
		}
	}
	None
}
fn size(mut value: usize, out: &mut Vec<u8>) {
	while value >= 128 {
		out.push((value as u8 & 127) | 128);
		value >>= 7;
	}
	out.push(value as u8);
}
fn header(obu: &[u8]) -> Option<usize> {
	let &h = obu.first()?;
	if h & 0x81 != 0 || matches!((h >> 3) & 15, 0 | 8) {
		return None;
	}
	if h & 4 != 0 {
		// Temporal scalability is permitted; extra spatial surfaces are not.
		if obu.get(1)? & 31 != 0 {
			return None;
		}
		Some(2)
	} else {
		Some(1)
	}
}
fn body(obu: &[u8]) -> Option<(usize, &[u8])> {
	let h = header(obu)?;
	if obu[0] & 2 == 0 {
		return Some((h, &obu[h..]));
	}
	let (len, count) = leb(&obu[h..])?;
	if h + count + len != obu.len() {
		return None;
	}
	Some((h, &obu[h + count..]))
}
fn reserve(out: &mut Vec<u8>, required: usize) {
	if required > out.capacity() && out.capacity().saturating_mul(2) > MAX_FRAME_BYTES {
		out.reserve_exact(required - out.len());
	}
}
fn write_obu(obu: &[u8], omit_size: bool, out: &mut Vec<u8>) -> Option<()> {
	let (h, payload) = body(obu)?;
	if out.len().checked_add(h + 8 + payload.len())? > MAX_FRAME_BYTES {
		return None;
	}
	reserve(out, out.len() + h + 8 + payload.len());
	out.push(if omit_size { obu[0] & !2 } else { obu[0] | 2 });
	out.extend_from_slice(&obu[1..h]);
	if !omit_size {
		size(payload.len(), out);
	}
	out.extend_from_slice(payload);
	Some(())
}

#[derive(Default)]
pub(super) struct Fragments {
	partial: Vec<u8>,
	units: usize,
}
impl Fragments {
	pub fn clear(&mut self) {
		self.partial.clear();
		self.units = 0;
		if self.partial.capacity() > 256 * 1024 {
			self.partial = Vec::new();
		}
	}
	pub fn pending(&self) -> bool {
		!self.partial.is_empty()
	}
	pub fn append(&mut self, data: &[u8], marker: bool, frame: &mut Vec<u8>) -> Result<(), ()> {
		let &h = data.first().ok_or(())?;
		let z = h & 128 != 0;
		let y = h & 64 != 0;
		let w = usize::from((h >> 4) & 3);
		if h & 7 != 0 || (h & 8 != 0 && z) || (marker && y) || z != self.pending() {
			return Err(());
		}
		let mut at = 1;
		let mut count = 0;
		while at < data.len() {
			count += 1;
			if count > 64 {
				return Err(());
			}
			let len = if w == 0 || count != w {
				let (len, n) = leb(&data[at..]).ok_or(())?;
				at += n;
				len
			} else {
				data.len() - at
			};
			let element = data
				.get(at..at.checked_add(len).ok_or(())?)
				.filter(|s| !s.is_empty())
				.ok_or(())?;
			at += len;
			if frame.len() + self.partial.len() + element.len() + 8 > MAX_FRAME_BYTES {
				return Err(());
			}
			let continues = count == 1 && z;
			if !continues && self.pending() {
				return Err(());
			}
			let required = self.partial.len() + element.len();
			reserve(&mut self.partial, required);
			self.partial.extend_from_slice(element);
			if at == data.len() && y {
				continue;
			}
			self.units += 1;
			if self.units > 256 {
				return Err(());
			}
			write_obu(&self.partial, marker && at == data.len(), frame).ok_or(())?;
			self.partial.clear();
		}
		if count == 0 || (w != 0 && w != count) {
			return Err(());
		}
		Ok(())
	}
}

fn visit_obus(mut data: &[u8], mut visit: impl FnMut(u8, &[u8]) -> Option<()>) -> Option<()> {
	let mut count = 0;
	while !data.is_empty() {
		count += 1;
		if count > 256 {
			return None;
		}
		let h = header(data)?;
		let (len, n) = if data[0] & 2 != 0 {
			leb(&data[h..])?
		} else {
			(data.len() - h, 0)
		};
		let end = h.checked_add(n)?.checked_add(len)?;
		visit((data[0] >> 3) & 15, data.get(h + n..end)?)?;
		data = data.get(end..)?;
	}
	(count > 0).then_some(())
}
#[derive(Default)]
pub(super) struct Info {
	pub keyframe: bool,
	pub sequence: bool,
}
pub(super) fn inspect(data: &[u8]) -> Option<Info> {
	let mut info = Info::default();
	let mut reduced = false;
	visit_obus(data, |kind, payload| {
		if kind == 1 {
			reduced = sequence(payload)?;
			info.sequence = true;
		}
		if matches!(kind, 3 | 6) {
			info.keyframe |= reduced || payload.first().is_some_and(|b| b & 0xf0 == 0x10);
		}
		Some(())
	})?;
	Some(info)
}
/// GStreamer's TU-aligned obu-stream needs a temporal delimiter, omitted by RTP,
/// and explicit sizes, including on the final plaintext OBU. Normalize after DAVE.
pub(super) fn normalize(mut data: &[u8]) -> Option<Vec<u8>> {
	let mut result = Vec::new();
	if (data.first()? >> 3) & 15 != 2 {
		result.extend_from_slice(&[0x12, 0]); // Empty, explicitly sized temporal delimiter.
	}
	while !data.is_empty() {
		let h = header(data)?;
		let (len, n) = if data[0] & 2 != 0 {
			leb(&data[h..])?
		} else {
			(data.len() - h, 0)
		};
		let end = h.checked_add(n)?.checked_add(len)?;
		write_obu(data.get(..end)?, false, &mut result)?;
		data = data.get(end..)?;
	}
	Some(result)
}

// AV1 5.5 sequence_header_obu: validate profile, maximum surface and colour before
// native submission. Decode only Main profile's 8/10-bit, non-monochrome 4:2:0.
fn sequence(data: &[u8]) -> Option<bool> {
	let mut b = Bits {
		data: data.get(..data.len().min(4096))?,
		at: 0,
	};
	if b.read(3)? != 0 {
		return None;
	}
	let still = b.read(1)? != 0;
	let reduced = b.read(1)? != 0;
	if reduced {
		if !still {
			return None;
		}
		b.read(5)?;
	} else {
		let mut delay_bits = None;
		if b.read(1)? != 0 {
			b.skip(64)?;
			if b.read(1)? != 0 {
				b.uvlc()?;
			}
			if b.read(1)? != 0 {
				delay_bits = Some(b.read(5)? as usize + 1);
				b.skip(42)?;
			}
		}
		let display = b.read(1)? != 0;
		let ops = b.read(5)?;
		for _ in 0..=ops {
			b.read(12)?;
			if b.read(5)? > 7 {
				b.read(1)?;
			}
			if let Some(n) = delay_bits
				&& b.read(1)? != 0
			{
				b.skip(n * 2 + 1)?;
			}
			if display && b.read(1)? != 0 {
				b.read(4)?;
			}
		}
	}
	let width_bits = b.read(4)? as usize + 1;
	let height_bits = b.read(4)? as usize + 1;
	let width = b.read(width_bits)? + 1;
	let height = b.read(height_bits)? + 1;
	platform::video::check_live_dimensions(width, height).ok()?;
	if !reduced && b.read(1)? != 0 {
		b.skip(7)?;
	}
	b.skip(3)?;
	if !reduced {
		b.skip(4)?;
		let order = b.read(1)? != 0;
		if order {
			b.skip(2)?;
		}
		let screen = if b.read(1)? != 0 { 2 } else { b.read(1)? };
		if screen > 0 && b.read(1)? == 0 {
			b.read(1)?;
		}
		if order {
			b.read(3)?;
		}
	}
	b.skip(3)?;
	b.read(1)?; // high_bitdepth, Main supports 8 or 10 bits.
	if b.read(1)? != 0 {
		return None;
	} // monochrome
	if b.read(1)? != 0 {
		let colour = (b.read(8)?, b.read(8)?, b.read(8)?);
		if colour == (1, 13, 0) {
			return None;
		} // RGB / 4:4:4
	}
	b.skip(1 + 2 + 1 + 1)?; // range, chroma position, separate UV, film grain
	Some(reduced)
}
struct Bits<'a> {
	data: &'a [u8],
	at: usize,
}
impl Bits<'_> {
	fn skip(&mut self, n: usize) -> Option<()> {
		if n > self.data.len().checked_mul(8)?.checked_sub(self.at)? {
			return None;
		}
		self.at += n;
		Some(())
	}
	fn read(&mut self, n: usize) -> Option<u32> {
		if n > 32 {
			return None;
		}
		let mut value = 0;
		for _ in 0..n {
			value =
				(value << 1) | u32::from((self.data.get(self.at / 8)? >> (7 - self.at % 8)) & 1);
			self.at += 1;
		}
		Some(value)
	}
	fn uvlc(&mut self) -> Option<()> {
		let mut n = 0;
		while self.read(1)? == 0 {
			n += 1;
			if n >= 32 {
				return Some(());
			}
		}
		self.skip(n)
	}
// Bounded single-layer AV1 RTP packetization, after codec-aware DAVE encryption.
//
// AOM AV1 RTP payload format v1.0.0 sections 4.4–5:
// https://aomediacodec.github.io/av1-rtp-spec/v1.0.0.html
// DAVE's AV1 transform removes the final OBU size; Discord's patched receiver
// preserves that omission so the authenticated frame, including its footer, survives:
// https://github.com/discord/dave-protocol/blob/main/protocol.md#av1
use crate::video::{MAX_FRAGMENTS, Packet};

const MAX_SOURCE_BYTES: usize = 2 * 1024 * 1024;
const MAX_DAVE_BYTES: usize = MAX_SOURCE_BYTES + 64 * 1024;
const MTU: usize = 1200;
// RTP header, transport tag/nonce, RTX original sequence and AV1 aggregation header.
const FRAGMENT_BYTES: usize = MTU - 12 - 20 - 2 - 1;
const INVALID: &str = "Invalid AV1 OBU frame";

struct Obu<'a> {
	header: u8,
	extension: Option<u8>,
	payload: &'a [u8],
	size_bytes: usize,
}
impl Obu<'_> {
	fn kind(&self) -> u8 {
		(self.header >> 3) & 15
	}
	fn discarded(&self) -> bool {
		matches!(self.kind(), 2 | 8 | 15)
	}
	fn wire_len(&self) -> usize {
		1 + usize::from(self.extension.is_some()) + self.payload.len()
	}
	/// Copy part of the RTP OBU representation without allocating a full-frame copy.
	fn fragment(&self, start: usize, end: usize, output: &mut Vec<u8>) {
		if start == 0 {
			output.push(self.header & !2);
		}
		if let Some(extension) = self.extension
			&& start <= 1
			&& end > 1
		{
			output.push(extension);
		}
		let header_len = 1 + usize::from(self.extension.is_some());
		if end > header_len {
			output.extend_from_slice(
				&self.payload[start.saturating_sub(header_len)..end - header_len],
			);
		}
	}
}

fn read_size(bytes: &[u8]) -> Result<(usize, usize), &'static str> {
	let mut size = 0u64;
	// AV1 leb128() is limited to eight bytes, including padded encoder output.
	for (index, byte) in bytes.iter().copied().take(8).enumerate() {
		size |= u64::from(byte & 127) << (index * 7);
		if byte & 128 == 0 {
			let size = usize::try_from(size).map_err(|_| INVALID)?;
			return Ok((size, index + 1));
		}
	}
	Err(INVALID)
}

fn write_size(mut size: usize, output: &mut Vec<u8>) {
	loop {
		let mut byte = (size & 127) as u8;
		size >>= 7;
		if size != 0 {
			byte |= 128;
		}
		output.push(byte);
		if size == 0 {
			break;
		}
	}
}

fn size_len(mut size: usize) -> usize {
	let mut length = 1;
	while size >= 128 {
		size >>= 7;
		length += 1;
	}
	length
}

fn obus(frame: &[u8], limit: usize) -> Result<Vec<Obu<'_>>, &'static str> {
	if frame.is_empty() || frame.len() > limit {
		return Err(INVALID);
	}
	let mut output = Vec::new();
	let mut at = 0;
	while at < frame.len() {
		if output.len() == MAX_FRAGMENTS {
			return Err("AV1 frame exceeds OBU limit");
		}
		let header = frame[at];
		at += 1;
		// Reject forbidden/reserved header bits and reserved OBU types.
		if header & 0x81 != 0 || !matches!((header >> 3) & 15, 1..=8 | 15) {
			return Err(INVALID);
		}
		let extension = if header & 4 != 0 {
			let extension = *frame.get(at).ok_or(INVALID)?;
			at += 1;
			// Low three extension bits are reserved by AV1.
			if extension & 7 != 0 {
				return Err(INVALID);
			}
			Some(extension)
		} else {
			None
		};
		let (length, size_bytes) = if header & 2 != 0 {
			let (length, size_bytes) = read_size(&frame[at..])?;
			at += size_bytes;
			(length, size_bytes)
		} else {
			(frame.len() - at, 0)
		};
		let end = at.checked_add(length).ok_or(INVALID)?;
		let payload = frame.get(at..end).ok_or(INVALID)?;
		if payload.is_empty() && !matches!((header >> 3) & 15, 2 | 8 | 15) {
			return Err(INVALID);
		}
		output.push(Obu {
			header,
			extension,
			payload,
			size_bytes,
		});
		at = end;
	}
	Ok(output)
}

/// Normalize bounded encoder OBU-stream output before `Codec::AV1` encryption.
/// In particular, trailing padding/temporal delimiters cannot prevent davey from
/// omitting the size of the last retained OBU. Canonical sizes survive RTP rewriting.
pub(crate) fn normalize_source(frame: &[u8]) -> Result<Vec<u8>, &'static str> {
	let retained: Vec<_> = obus(frame, MAX_SOURCE_BYTES)?
		.into_iter()
		.filter(|obu| !obu.discarded())
		.collect();
	if retained.is_empty() {
		return Err(INVALID);
	}
	let mut output = Vec::with_capacity(frame.len());
	for (index, obu) in retained.iter().enumerate() {
		let last = index + 1 == retained.len();
		output.push((obu.header & !2) | if last { 0 } else { 2 });
		if let Some(extension) = obu.extension {
			output.push(extension);
		}
		if !last {
			write_size(obu.payload.len(), &mut output);
		}
		output.extend_from_slice(obu.payload);
	}
	if output.len() > MAX_SOURCE_BYTES {
		return Err("AV1 frame exceeds the sharing limit");
	}
	Ok(output)
}

/// Admit encoder samples before they enter the bounded screen transport queue.
pub(crate) fn validate_source(frame: &[u8]) -> Result<(), &'static str> {
	let obus = obus(frame, MAX_SOURCE_BYTES)?;
	if !obus.iter().any(|obu| matches!(obu.kind(), 3 | 6)) {
		return Err(INVALID);
	}
	Ok(())
}

/// Packetize the transformed DAVE frame. One OBU or OBU fragment per packet uses
/// W=1, avoiding extra length fields and keeping the final DAVE footer in that OBU.
pub(crate) fn packetize(
	frame: &[u8],
	sequence: &mut u16,
	timestamp: u32,
	ssrc: u32,
	payload_type: u8,
	new_sequence: bool,
) -> Result<Vec<Packet>, &'static str> {
	if payload_type > 127 || ssrc == 0 {
		return Err("Invalid AV1 RTP configuration");
	}
	let obus = obus(frame, MAX_DAVE_BYTES)?;
	// DAVE authenticates the OBU headers and sizes. A receiver must reconstruct
	// canonical sizes except on the final OBU, which includes non-AV1 footer bytes.
	if obus.last().is_none_or(|obu| obu.header & 2 != 0)
		|| obus.iter().any(|obu| {
			obu.discarded()
				|| (obu.size_bytes != 0 && obu.size_bytes != size_len(obu.payload.len()))
		}) {
		return Err(INVALID);
	}
	let count: usize = obus
		.iter()
		.map(|obu| obu.wire_len().div_ceil(FRAGMENT_BYTES))
		.sum();
	if count > MAX_FRAGMENTS {
		return Err("AV1 frame exceeds fragment limit");
	}
	let new_sequence = new_sequence && obus.first().is_some_and(|obu| obu.kind() == 1);
	let mut packets = Vec::with_capacity(count);
	for obu in obus {
		let mut at = 0;
		while at < obu.wire_len() {
			let end = (at + FRAGMENT_BYTES).min(obu.wire_len());
			let first = packets.is_empty();
			let last = packets.len() + 1 == count;
			let mut payload = Vec::with_capacity(end - at + 1);
			payload.push(
				0x10 // W=1: the sole element has no aggregation length field.
					| if at != 0 { 0x80 } else { 0 } // Z: continuation from previous packet.
					| if end != obu.wire_len() { 0x40 } else { 0 } // Y: continues in next.
					| if first && new_sequence { 0x08 } else { 0 },
			);
			obu.fragment(at, end, &mut payload);
			let mut header = [0; 12];
			header[0] = 0x80;
			header[1] = payload_type | if last { 0x80 } else { 0 };
			header[2..4].copy_from_slice(&sequence.to_be_bytes());
			header[4..8].copy_from_slice(&timestamp.to_be_bytes());
			header[8..12].copy_from_slice(&ssrc.to_be_bytes());
			*sequence = sequence.wrapping_add(1);
			packets.push(Packet { header, payload });
			at = end;
		}
	}
	Ok(packets)
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn aggregate_and_fragment_rebuild_dave_framing_before_plaintext_normalization() {
		let mut fragments = Fragments::default();
		let mut frame = Vec::new();
		fragments
			.append(&[0x60, 2, 0x28, 0x55, 0x30, 0x10], false, &mut frame)
			.unwrap();
		fragments
			.append(&[0x90, 0xaa, 0xbb], true, &mut frame)
			.unwrap();
		assert_eq!(frame, [0x2a, 1, 0x55, 0x30, 0x10, 0xaa, 0xbb]);
		assert!(inspect(&frame).unwrap().keyframe);
		assert_eq!(
			normalize(&frame).unwrap(),
			[0x12, 0, 0x2a, 1, 0x55, 0x32, 3, 0x10, 0xaa, 0xbb]
		);
		let native = normalize(&frame).unwrap();
		assert_eq!(normalize(&native).unwrap(), native); // Preserve existing TU delimiters.
		for payload in [
			&[0x90, 0x30, 7][..], // Continuation without a fragment.
			&[0x50, 0x30, 7],     // Marker on an unfinished fragment.
			&[0, 0x80],           // Truncated element length.
			&[0x30, 1, 0x28],     // Wrong element count.
			&[0x10, 0x34, 8, 7],  // Extra spatial layer.
			&[0x10, 0xb0, 7],     // Forbidden OBU bit.
			&[0x10, 0x40, 7],     // Tile lists are outside AV1 RTP reception.
			&[0x10, 0x32, 5, 7],  // Inconsistent internal size.
			&[0x10],              // No element.
		] {
			assert!(
				Fragments::default()
					.append(payload, true, &mut Vec::new())
					.is_err()
			);
		}
	}
	#[test]
	fn fragment_bytes_and_frame_obu_counts_are_bounded_before_copying() {
		let mut fragments = Fragments::default();
		let mut frame = Vec::new();
		fragments
			.append(&[0x50, 0x30, 7], false, &mut frame)
			.unwrap();
		let mut middle = vec![7; 4096];
		middle[0] = 0xd0;
		while fragments.append(&middle, false, &mut frame).is_ok() {}
		assert!(frame.len() + fragments.partial.len() <= MAX_FRAME_BYTES);
		assert!(fragments.partial.capacity() <= MAX_FRAME_BYTES);
		fragments.clear();
		let packet: Vec<_> = std::iter::once(0).chain([1, 0x28].repeat(64)).collect();
		for _ in 0..4 {
			fragments.append(&packet, false, &mut frame).unwrap();
		}
		assert!(fragments.append(&[0x10, 0x28], true, &mut frame).is_err());
	}
	fn reduced(width: u32, height: u32, mono: bool) -> Vec<u8> {
		let mut bits = Vec::new();
		let mut put = |v: u32, n: usize| bits.extend((0..n).rev().map(|i| v & (1 << i) != 0));
		put(0, 3);
		put(1, 1);
		put(1, 1);
		put(0, 5);
		put(15, 4);
		put(15, 4);
		put(width - 1, 16);
		put(height - 1, 16);
		put(0, 3 + 3);
		put(0, 1);
		put(u32::from(mono), 1);
		put(0, 1);
		put(0, 5);
		put(1, 1);
		bits.chunks(8)
			.map(|c| {
				c.iter()
					.enumerate()
					.fold(0, |v, (i, b)| v | (u8::from(*b) << (7 - i)))
			})
			.collect()
	}
	#[test]
	fn sequence_bounds_reject_unsupported_surfaces_and_truncated_colour_syntax() {
		assert_eq!(sequence(&reduced(2560, 1440, false)), Some(true));
		assert_eq!(sequence(&reduced(3840, 2160, false)), Some(true));
		assert_eq!(sequence(&reduced(2160, 3840, false)), Some(true));
		assert!(sequence(&reduced(3840, 3840, false)).is_none());
		assert!(sequence(&reduced(4096, 2160, false)).is_none());
		assert!(sequence(&reduced(640, 480, true)).is_none());
		let valid = reduced(2560, 1440, false);
		for end in 0..valid.len() - 1 {
			assert!(sequence(&valid[..end]).is_none());
		}
	}
	use crate::{crypto::Dave, test_mls::Delivery};
	// One 64×64 black keyframe produced with libaom/GStreamer av1enc, then decoded
	// with av1dec offline. It includes a temporal delimiter, sequence header and frame.
	const BLACK_KEYFRAME: &[u8] = &[
		0x12, 0x00, 0x0a, 0x0b, 0x00, 0x00, 0x00, 0x02, 0xaf, 0xff, 0xf0, 0x36, 0xbe, 0x40, 0x10,
		0x32, 0x10, 0x10, 0x80, 0x80, 0x01, 0x00, 0x00, 0xb4, 0x51, 0xb4, 0xf0, 0xa1, 0xf1, 0x97,
		0xe0, 0x13, 0x24,
	];

	/// Discord's AV1 receiver restores sizes except for the last reconstructed OBU.
	fn depacketize(packets: &[Packet]) -> Vec<u8> {
		let mut obus = Vec::<Vec<u8>>::new();
		let mut fragmented = false;
		for packet in packets {
			let header = packet.payload[0];
			assert_eq!(header & 0x30, 0x10);
			assert_eq!(header & 0x80 != 0, fragmented);
			if !fragmented {
				obus.push(Vec::new());
			}
			obus.last_mut()
				.unwrap()
				.extend_from_slice(&packet.payload[1..]);
			fragmented = header & 0x40 != 0;
		}
		assert!(!fragmented);
		let mut frame = Vec::new();
		for (index, obu) in obus.iter().enumerate() {
			let last = index + 1 == obus.len();
			let header_len = 1 + usize::from(obu[0] & 4 != 0);
			frame.push(obu[0] | if last { 0 } else { 2 });
			frame.extend_from_slice(&obu[1..header_len]);
			if !last {
				write_size(obu.len() - header_len, &mut frame);
			}
			frame.extend_from_slice(&obu[header_len..]);
		}
		frame
	}

	#[test]
	fn source_normalization_drops_trailing_padding_and_canonicalizes_sizes() {
		let frame = [0x12, 0, 0x0a, 0x81, 0, 7, 0x36, 0, 2, 8, 9, 0x7a, 1, 0];
		assert_eq!(
			normalize_source(&frame).unwrap(),
			[0x0a, 1, 7, 0x34, 0, 8, 9]
		);
		assert!(normalize_source(&[0x12, 0, 0x7a, 0]).is_err());
	}

	#[test]
	fn rtp_fragmentation_preserves_headers_footer_and_sequence_wrap() {
		let mut frame = vec![0x0a, 1, 7, 0x34, 0];
		frame.resize(FRAGMENT_BYTES * 2 + 90, 5);
		let mut sequence = u16::MAX;
		let packets = packetize(&frame, &mut sequence, 90_000, 42, 103, true).unwrap();
		assert_eq!(packets.len(), 4);
		assert_eq!(sequence, 3);
		assert_eq!(packets[0].payload[0], 0x18);
		assert_eq!(packets[1].payload[0], 0x50);
		assert_eq!(packets[2].payload[0], 0xd0);
		assert_eq!(packets[3].payload[0], 0x90);
		assert_eq!(packets[3].header[1], 103 | 0x80);
		assert!(packets[..3].iter().all(|packet| packet.header[1] == 103));
		assert!(
			packets
				.iter()
				.all(|packet| packet.payload.len() + 12 + 20 + 2 <= MTU)
		);
		assert_eq!(depacketize(&packets), frame);
	}

	#[test]
	fn malformed_and_oversized_obus_fail_before_changing_sequence() {
		for frame in [
			vec![],
			vec![0xb0, 1],
			vec![0x35, 0, 1],
			vec![0x34],
			vec![0x34, 7, 1],
			vec![0x32, 0xff, 0xff],
			vec![0x32, 9, 1],
			vec![0x32, 0x81, 0, 1],
			vec![0x32, 1, 1],
			vec![0x78, 1],
			vec![0x30; MAX_DAVE_BYTES + 1],
		] {
			let mut sequence = 77;
			assert!(packetize(&frame, &mut sequence, 0, 42, 103, true).is_err());
			assert_eq!(sequence, 77);
		}
		let mut over_obus = [0x32, 0].repeat(MAX_FRAGMENTS);
		over_obus.push(0x30);
		assert!(normalize_source(&over_obus).is_err());
	}

	#[test]
	fn dave_av1_rtp_round_trip_preserves_authenticated_frame() {
		let server = Delivery::new();
		let mut alice = Dave::new(1, Some(2), 3).unwrap();
		let mut bob = Dave::new(2, Some(1), 3).unwrap();
		alice.session.set_external_sender(&server.external).unwrap();
		bob.session.set_external_sender(&server.external).unwrap();
		let (commit, welcome) = server.add(&mut alice, &bob.key_package().unwrap());
		let mut committed = vec![0, 5];
		committed.extend(commit);
		let mut welcomed = vec![0, 5];
		welcomed.extend(welcome);
		alice.group_changed(29, &committed).unwrap();
		bob.group_changed(30, &welcomed).unwrap();
		alice.execute(5).unwrap();
		bob.execute(5).unwrap();

		let mut original = vec![0x0a, 1, 7, 0x30];
		original.resize(FRAGMENT_BYTES * 2 + 30, 9);
		for source in [BLACK_KEYFRAME, original.as_slice()] {
			validate_source(source).unwrap();
			let original = normalize_source(source).unwrap();
			let encrypted = alice
				.session
				.encrypt(davey::MediaType::VIDEO, davey::Codec::AV1, &original)
				.unwrap()
				.into_owned();
			let packets = packetize(&encrypted, &mut 0, 90_000, 11, 103, true).unwrap();
			let restored = depacketize(&packets);
			assert_eq!(restored, encrypted);
			assert_eq!(
				bob.session
					.decrypt(1, davey::MediaType::VIDEO, &restored)
					.unwrap(),
				original
			);
		}
	}
}
