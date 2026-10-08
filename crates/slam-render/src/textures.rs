//! Textures addressed by small copyable ids, so that sprites recorded for a frame
//! name their texture without borrowing it.

/// Key of a texture in a [`Textures`] set, or any other id the caller resolves when
/// drawing (see [`crate::sprite::SpriteRenderer::draw`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureId(pub u32);

/// Owns textures and hands out [`TextureId`]s. Ids of removed textures are reused.
pub struct Textures<T> {
    slots: Vec<Option<T>>,
    free: Vec<u32>,
}

impl<T> Default for Textures<T> {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
        }
    }
}

impl<T> Textures<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, texture: T) -> TextureId {
        match self.free.pop() {
            Some(index) => {
                self.slots[index as usize] = Some(texture);
                TextureId(index)
            }
            None => {
                let index = u32::try_from(self.slots.len()).expect("fewer than 2^32 textures");
                self.slots.push(Some(texture));
                TextureId(index)
            }
        }
    }

    /// Takes the texture out (to destroy it); its id may be handed out again.
    pub fn remove(&mut self, id: TextureId) -> Option<T> {
        let texture = self.slots.get_mut(id.0 as usize)?.take()?;
        self.free.push(id.0);
        Some(texture)
    }

    pub fn get(&self, id: TextureId) -> Option<&T> {
        self.slots.get(id.0 as usize)?.as_ref()
    }

    /// Removes every texture, e.g. to destroy them all.
    pub fn drain(&mut self) -> impl Iterator<Item = T> + '_ {
        self.free.clear();
        self.slots.drain(..).flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inserts_gets_and_reuses_ids() {
        let mut t = Textures::new();
        let a = t.insert('a');
        let b = t.insert('b');
        assert_ne!(a, b);
        assert_eq!(t.get(a), Some(&'a'));
        assert_eq!(t.remove(a), Some('a'));
        assert_eq!(t.get(a), None);
        assert_eq!(t.remove(a), None);
        let c = t.insert('c');
        assert_eq!(c, a);
        assert_eq!(t.get(c), Some(&'c'));
        assert_eq!(t.get(TextureId(99)), None);
        let mut all: Vec<_> = t.drain().collect();
        all.sort();
        assert_eq!(all, ['b', 'c']);
        assert_eq!(t.get(b), None);
    }
}
