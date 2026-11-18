# Third-party software and references

The Verdandi fork provides the [GNU GPL version 3 license text](../LICENSE.txt).
The bridge links the vendored Encdr crate. Cargo resolves the bridge's other
Rust dependencies using [Cargo.lock](Cargo.lock). Each dependency keeps its own
license and notices.

| Component | How it is used | License and source |
| --- | --- | --- |
| Encdr 0.8.5 by RufusIbiza and contributors | Vendored Rust dependency for controller discovery, screen rendering and NI service communication | GPL-3.0-or-later. See the [original license](vendor/encdr/LICENSE), [dated modification notice](vendor/encdr/NOTICE.md), and [upstream source](https://github.com/RufusIbiza/Encdr/tree/4e1fa75aa329fc18823be88e6a0c1b385f53f6ac). |
| Rebellion by Björn Kalkbrenner and contributors | Reference for the NI service LED message format and hardware LED indices | The two referenced files carry LGPLv3 notices. The bridge implements the protocol in Rust and does not bundle Rebellion. See [the LED protocol](https://github.com/terminar/rebellion/blob/59933a370f24fb897c81b6ba82c05ffdacb454f3/scripts/niproto.lua#L926-L997) and [device mappings](https://github.com/terminar/rebellion/blob/59933a370f24fb897c81b6ba82c05ffdacb454f3/scripts/mappings.lua#L385-L423). |
| Native Instruments hardware services | macOS services that the bridge connects to for NI instance switching | Installed separately from Native Instruments. This repository does not distribute the agents or firmware. |

For a binary release, consult [GPLv3 section 6](https://www.gnu.org/licenses/gpl-3.0.html#section6)
and include the license notices and corresponding source for the exact bridge
build. Use the lockfile to identify the Cargo dependencies for that build.
