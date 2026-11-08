/// A factory preset: a name and plain values for some parameters, keyed by
/// stable parameter key. Parameters not listed keep their defaults.
#[derive(Clone, Copy, Debug)]
pub struct Preset {
    pub name: &'static str,
    pub params: &'static [(&'static str, f32)],
}
