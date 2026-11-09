# Asset licenses

Every bundled asset, its source and its license. Add a row for each new
asset in the same commit that adds it.

| Asset | Source | Version | License | Use |
| --- | --- | --- | --- | --- |
| `fonts/barlow/BarlowSemiCondensed-*.ttf` | [google/fonts `ofl/barlowsemicondensed`](https://github.com/google/fonts/tree/main/ofl/barlowsemicondensed), Jeremy Tribby | main, fetched 2026-11-09 | SIL OFL 1.1 (`fonts/barlow/OFL.txt`) | Interface text |
| `icons/peel-*.png` | Drawn by `scripts/make-icon.py` for this project | — | GPL-3.0-or-later (project artwork) | App and window icon |
| `fonts/phosphor/Phosphor.ttf`, `Phosphor-Fill.ttf` | [`@phosphor-icons/web`](https://github.com/phosphor-icons/web) | 2.1.2 | MIT (`fonts/phosphor/LICENSE`) | Interface icons |
| `drums/big-rusty/*.wav` (96 recordings) | [Karoryfer Samples, Big Rusty Drums](https://shop.karoryfer.com/pages/free-big-rusty-drums), [source](https://github.com/sfzinstruments/karoryfer.big-rusty-drums) | `f07ce00df34a46b6b08375be56fe116cf15782bc`, fetched 2026-11-10; source files and processing in `drums/big-rusty/manifest.json` | CC0-1.0 (`drums/big-rusty/LICENSE.txt`) | Embedded recorded acoustic kit |

| `scripts/*.js`, `scripts/*.lua`, `scripts/*.py` | Written for Peel Studio | API 1 | GPL-3.0-or-later (project code) | Factory scripting actions |

Release packages also include a private CPython runtime and Python packages:

| Component | Version | Notices |
| --- | --- | --- |
| CPython / python-build-standalone | 3.12.13 / 20260414 | PSF and component notices in the runtime's LICENSE files and `licenses/`; source notices are in `packaging/python/licenses/` |
| PyTheory | 0.57.13 | MIT; retained in its `.dist-info` metadata |
| NumPy, SciPy, sounddevice, CFFI, Rich and their dependencies | Pinned in `packaging/python/requirements.lock` | Each wheel retains its own license files and notices |

See [packaging/python/README.md](../packaging/python/README.md) for runtime
sources, dependency pins and build instructions.

The Acoustic Kit uses bundled recordings. The other native instruments
synthesize their factory sounds at runtime.
