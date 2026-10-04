use std::{collections::{HashMap, HashSet}, io::{self, Write}, sync::{Arc, Weak}};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use crate::image::{ImagePlacement, ImagePlacementKey, ImageSource, ImageSourceId};

/// Stateful Kitty protocol adapter; scene identities remain opaque.
#[derive(Debug, Default)]
pub struct KittyImageBackend {
    next_image: u32,
    next_placement: u32,
    sources: HashMap<ImageSourceId, (u32, Weak<ImageSource>)>,
    placements: HashMap<ImagePlacementKey, (u32, ImageSourceId)>,
}

impl KittyImageBackend {
    /// Deletes visible placements while retaining uploads that may be reused.
    pub fn clear(&mut self, out: &mut impl Write) -> io::Result<()> {
        for (_, (placement, source)) in self.placements.drain() {
            Self::command(out, &format!("a=d,d=i,i={},p={},q=2", self.sources[&source].0, placement), b"")?;
        }
        Ok(())
    }

    /// Deletes uploads before leaving the alternate screen.
    pub fn shutdown(&mut self, out: &mut impl Write) -> io::Result<()> {
        self.clear(out)?;
        for (_, (image, _)) in self.sources.drain() {
            Self::command(out, &format!("a=d,d=I,i={},q=2", image), b"")?;
        }
        Ok(())
    }
}

impl KittyImageBackend {
    fn id(next: &mut u32) -> io::Result<u32> {
        *next = next.checked_add(1).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Kitty image ID exhausted"))?;
        Ok(*next)
    }
    fn command(out: &mut impl Write, control: &str, payload: &[u8]) -> io::Result<()> {
        write!(out, "\x1b_G{};", control)?;
        out.write_all(payload)?;
        out.write_all(b"\x1b\\")
    }
    fn upload(&mut self, out: &mut impl Write, source: &Arc<ImageSource>) -> io::Result<u32> {
        if let Some(&(id, _)) = self.sources.get(&source.id) { return Ok(id) }
        let id = Self::id(&mut self.next_image)?;
        let encoded = STANDARD.encode(&source.rgba);
        for (part, chunk) in encoded.as_bytes().chunks(4096).enumerate() {
            let control = if part == 0 { format!("a=t,t=d,f=32,s={},v={},i={},q=2,m={}", source.width, source.height, id, if chunk.len() == encoded.len() { 0 } else { 1 }) } else { format!("m={},q=2", if (part + 1) * 4096 >= encoded.len() { 0 } else { 1 }) };
            Self::command(out, &control, chunk)?;
        }
        self.sources.insert(source.id, (id, Arc::downgrade(source)));
        Ok(id)
    }
    pub fn reconcile(&mut self, out: &mut impl Write, placements: &[ImagePlacement]) -> io::Result<()> {
        let mut seen = HashSet::new();
        for placement in placements {
            if !seen.insert(placement.key) || placement.crop.width == 0 || placement.crop.height == 0 || placement.cell_rect.width == 0 || placement.cell_rect.height == 0 || placement.crop.x.checked_add(placement.crop.width).is_none_or(|x| x > placement.source.width) || placement.crop.y.checked_add(placement.crop.height).is_none_or(|y| y > placement.source.height) { return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid Kitty placement")); }
        }
        for placement in placements {
            let image = self.upload(out, &placement.source)?;
            let old = self.placements.get(&placement.key).copied();
            if old.is_some_and(|(_, source)| source != placement.source.id) {
                let (old_placement, old_source) = old.unwrap();
                let old_image = self.sources[&old_source].0;
                Self::command(out, &format!("a=d,d=i,i={},p={},q=2", old_image, old_placement), b"")?;
            }
            let placement_id = old.map_or_else(|| Self::id(&mut self.next_placement), |(id, _)| Ok(id))?;
            let r = placement.cell_rect;
            let c = placement.crop;
            write!(out, "\x1b[s\x1b[{};{}H", r.y + 1, r.x + 1)?;
            Self::command(out, &format!("a=p,i={},p={},x={},y={},w={},h={},c={},r={},C=1,q=2,z=0", image, placement_id, c.x, c.y, c.width, c.height, r.width, r.height), b"")?;
            self.placements.insert(placement.key, (placement_id, placement.source.id));
        }
        let desired: HashSet<_> = placements.iter().map(|p| p.key).collect();
        self.placements.retain(|key, value| {
            if desired.contains(key) { return true }
            let (placement, source) = *value;
            let _ = Self::command(out, &format!("a=d,d=i,i={},p={},q=2", self.sources[&source].0, placement), b"");
            false
        });
        write!(out, "\x1b[u")
    }
}
