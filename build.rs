use anyhow::Result;
use vergen::{vergen, Config};
use vergen::{ShaKind, TimestampKind};

fn main() -> Result<()> {
    let mut config = Config::default();
    *config.build_mut().kind_mut() = TimestampKind::All;
    *config.git_mut().sha_kind_mut() = ShaKind::Short;
    vergen(config)
}
