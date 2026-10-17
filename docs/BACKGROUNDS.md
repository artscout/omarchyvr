# VR backgrounds

The environment around you in the headset: a 360-degree picture drawn by WayVR behind the screens (its skybox).
It is not the desktop wallpaper (`Max, next wallpaper` is Omarchy's and unrelated).

## Use it
- **App**: the Backgrounds page. A thumbnail grid by theme; tap one to show it (right away if Work runs). Resolution:
  Auto (the best the picture has, up to 8K), 8K, 4K, 2K. **Off / passthrough** brings the room back.
  **Download more** fetches the free pictures (resumable; progress shown).
- **Voice**: "Max, set background flowers" (universe, ocean, hawaii, sunset, mountains, forest, desert, night sky,
  city night, snow, lake, underwater ...), "Max, next background", "Max, previous background", "Max, background off".
  The voice names are `<theme>_<picture>` (`omarchyvr-backgrounds list`); a theme word shows its first picture,
  "next background" steps through all of them.
- **Work start**: when a background is on, `omarchyvr desk start` shows it once WayVR runs (over the preset's passthrough).
- The choice is one file, `~/.config/omarchyvr/background.toml` (`enabled`, `id`, `res`), shared by the app, voice and the
  desk start.

## How it works
| | |
|---|---|
| Library | `~/.local/share/omarchyvr/backgrounds/<theme>/` (never in git): `manifest.toml`, `thumbs/<name>.png`, `<name>_<8k\|4k\|2k>.dds` |
| Format | equirectangular 2:1, DDS (WayVR's skybox reads DDS only; BC1/BC7 stay compressed on the GPU: 8K = 16 to 32 MB) |
| WayVR | `wayvrctl skybox <path to .dds>` (the equirect layer of `skybox.rs`, `khr_composition_layer_equirect2`); the passthrough camera goes off after the picture is drawn |
| Sizes | per picture 8K / 4K / 2K as far as the source goes (a 6000 px source has 4K and 2K) |
| Source list | `rust/omarchyvr-backgrounds/sources.toml` (URLs and licences; no images in the repo) |
| Downloader | `omarchyvr-backgrounds download`: official APIs only (Poly Haven, Wikimedia Commons), `curl -C -` resume, size and decode check, SHA-256 of every file in the manifest, 4 GB cap, 2 GB free-disk guard, `nice -n 19`; `omarchyvr-backgrounds verify [--deep]` checks the library again |
| Engine | `curl` and ImageMagick (`magick`) |
| First set | the 16 pictures of WayVR's `skybox/` folder (`CREDITS.txt`, BC7) are linked into the library, not copied; sources.toml skips what they already cover |

CLI (for agents): `omarchyvr-backgrounds list|download|verify|set NAME|next|previous|off|res auto|8k|4k|2k|apply|status`.

## Ocean (animated)
A live sea and sky drawn by WayVR behind your screens in Work (research and measurements: docs/research/web-ocean.md;
inspired by github.com/Noniv/web-ocean, MIT, no code taken).
- **App**: Backgrounds page > Animated > "Ocean (animated)" (a switch like any picture: tap it, tap another picture or
  Off to leave it) with **Quality: Low / Medium / High**. **Voice** (2026-10-17): "Max, animated ocean background", "moving ocean", "live sea", "ocean animated high", and
  plain "ocean", "the sea" or "waves" (they prefer the animated ocean; "beach" and the picture names stay photos). It is
  in the voice's background list as `ocean:<quality>` (the quality in use, else the saved one), runs `wayvrctl skybox
  ocean:<quality>`, is saved to `background.toml` (`id = "ocean/animated"`, `ocean_quality`) and shows in the voice state. CLI: `omarchyvr-backgrounds ocean [low|medium|high] --apply`.
- **Saved**: `background.toml` gets `id = "ocean/animated"` and `ocean_quality = "low|medium|high"`; the Work start applies
  it like a picture (`wayvrctl skybox ocean:<quality>`; no file needed, nothing to download).
- **Only in Work**: it is a WayVR skybox, and WayVR runs only in Work (Play is SteamVR; the setting is not applied there).
- **What you see**: a sea under a golden-hour sky, the horizon a few degrees below eye level so it clears the screens, a
  sun glitter path, clouds drifting, whitecaps on the steeper waves. The water is fixed in the room 3 m under the floor
  and the waves are anchored to the room; the picture follows your head position, so it has no parallax.
- **How**: `forks/wayvr/wayvr/src/ocean/` (waves, uniforms, renderer, tests), `shaders/ocean.frag`,
  `backend/openxr/ocean_sky.rs` and a hook in `skybox.rs`: a sum of 20/40/64 directional waves from a JONSWAP spectrum
  (Tessendorf, Pierson-Moskowitz, JONSWAP), Fresnel sky reflection, GGX sun glitter with the roughness of the waves too
  small to draw, Jacobian foam, a closed-form atmosphere (Nishita) with fbm clouds; drawn into the equirect swapchain
  every 2nd frame (every 3rd on Low). No compute passes, no textures, CPU about zero.
- **Cost (RTX 4090, headless, per drawn frame)**: Low 0.06 ms (2048x1024), Medium 0.38 ms (4096x2048), High 1.20 ms
  (6144x3072). Check in the headset with the wrist readout (`miss N /5s` must stay 0).
- **Test**: `cargo test -p wayvr ocean::` (the GPU part skips itself while `monado-service`, `vrserver` or
  `vrcompositor` runs, or with `OMARCHYVR_OCEAN_NOGPU=1`); `OMARCHYVR_OCEAN_OUT=DIR` keeps the PNGs (the 360 image and
  three views per quality) and `ocean-gpu-time.csv`. Pictures: docs/img/ocean-high-view-sun.png,
  docs/img/ocean-high-view-away.png, docs/img/backgrounds-ocean-page.png.

## Terms of the sources
- **Poly Haven**: CC0. Official API (`api.polyhaven.com`) with a named User-Agent; the 8K tonemapped JPG is downloaded
  once per picture, no bulk mirroring (about 40 files).
- **Wikimedia Commons**: files of CC0, public domain, CC BY and CC BY-SA only (read from the Commons API; others are
  refused). Original files fetched one by one with a named User-Agent, 1.5 s apart. CC BY / CC BY-SA need the credit:
  the manifest keeps it and the app shows it when you hover a picture. Personal use, nothing is redistributed.
- **NASA / ESA / ESO / NOIRLab**: used through their Commons copies (ESO, NOIRLab, Gaia are CC BY 4.0 / CC BY-SA 3.0
  IGO). NASA's own star-map download was too small (1024 px) and is not used.
- Not used: Unsplash, Pexels, Pixabay (flat photos; no 360 and their terms forbid redistribution as a collection).
  Flat photos are skipped: WayVR's skybox is an equirect layer, a flat picture would be stretched around you.

## Gaps
- No free-licence aurora panorama and no Hawaiian beach panorama was found; "hawaii" has Maunakea (summit, sunset,
  Milky Way) plus a tropical beach (Poly Haven's Secluded Beach, South Africa, from the first set).
- The two reef panoramas (Catlin Seaview) are 9500 px wide, the underwater theme is small (2 pictures).
- WayVR draws the picture as it is: no rotation or brightness control per picture yet.

## Picture list and licences
| Theme | Picture | Author | Licence | Sizes | Source |
|---|---|---|---|---|---|
| city | City night | Greg Zaal, Rico Cilliers | CC0 1.0 | 8K/4K/2K | https://polyhaven.com/a/vignaioli_night |
| city | Cobblestone Street Night | Greg Zaal, Jenelle van Heerden | CC0 | 8K/4K/2K | https://polyhaven.com/a/cobblestone_street_night |
| city | Donkin Reserve | Jenelle van Heerden | CC0 | 8K/4K/2K | https://polyhaven.com/a/donkin_reserve |
| city | Golden Bay | Greg Zaal | CC0 | 8K/4K/2K | https://polyhaven.com/a/golden_bay |
| city | Shanghai Bund | Greg Zaal | CC0 | 8K/4K/2K | https://polyhaven.com/a/shanghai_bund |
| desert | Aarfontein Sunrise | Dario Barresi | CC0 | 8K/4K/2K | https://polyhaven.com/a/aarfontein_sunrise |
| desert | Desert valley | Dario Barresi | CC0 1.0 | 8K/4K/2K | https://polyhaven.com/a/valley_of_desolation |
| desert | Goegap | Greg Zaal | CC0 | 8K/4K/2K | https://polyhaven.com/a/goegap |
| desert | Klippad Sunrise 2 | Greg Zaal | CC0 | 8K/4K/2K | https://polyhaven.com/a/klippad_sunrise_2 |
| flowers | Dutch flower fields | Wdejager | CC BY-SA 4.0 | 8K/4K/2K | https://commons.wikimedia.org/wiki/File:Dutch_flower_fields_(360_degree_aerial_pano).jpg |
| flowers | Flower Hillside | Andreas Mischok | CC0 | 8K/4K/2K | https://polyhaven.com/a/flower_hillside |
| flowers | Flower road | Greg Zaal | CC0 1.0 | 8K/4K/2K | https://polyhaven.com/a/flower_road |
| flowers | Rapeseed field in bloom | Maximilian Schönherr | CC BY-SA 4.0 | 4K/2K | https://commons.wikimedia.org/wiki/File:Field_of_Rape_in_Full_Flower_%E2%80%93_360%C2%B0_Drone_Panorama.jpg |
| flowers | Rose garden | Dimitrios Savva, Jarod Guest | CC0 1.0 | 8K/4K/2K | https://polyhaven.com/a/sunny_rose_garden |
| flowers | Sunflower meadow | G. Edward Johnson | CC BY 4.0 | 4K/2K | https://commons.wikimedia.org/wiki/File:Sunflowers_McKee-Beshers_WMA_360_pano_2025-07-25_09-13-17_1.jpg |
| flowers | Sunflowers | Sergej Majboroda | CC0 | 8K/4K/2K | https://polyhaven.com/a/sunflowers |
| flowers | Wildflower Field | Alexander Scholten | CC0 | 8K/4K/2K | https://polyhaven.com/a/wildflower_field |
| forest | Autumn Forest 04 | Alexander Scholten | CC0 | 8K/4K/2K | https://polyhaven.com/a/autumn_forest_04 |
| forest | Autumn field | Sergej Majboroda | CC0 1.0 | 8K/4K/2K | https://polyhaven.com/a/autumn_field |
| forest | Bamboo Tunnel | Dimitrios Savva, Jarod Guest | CC0 | 8K/4K/2K | https://polyhaven.com/a/bamboo_tunnel |
| forest | Birchwood | Sergej Majboroda | CC0 | 8K/4K/2K | https://polyhaven.com/a/birchwood |
| forest | Crystal Falls | Greg Zaal | CC0 | 8K/4K/2K | https://polyhaven.com/a/crystal_falls |
| forest | Forest Grove | Dimitrios Savva, Jarod Guest | CC0 | 8K/4K/2K | https://polyhaven.com/a/forest_grove |
| forest | Rainforest trail | Dimitrios Savva, Jarod Guest | CC0 1.0 | 8K/4K/2K | https://polyhaven.com/a/rainforest_trail |
| hawaii | Gemini North and the Milky Way | International Gemini Observatory/NOIRLab/NSF/AURA/ T. Slovinský | CC BY 4.0 | 8K/4K/2K | https://commons.wikimedia.org/wiki/File:Gemini_North_and_the_Milky_Way_360_Panorama_(MaunaKea_GeminiNorth_MW360Pano-2-CC).jpg |
| hawaii | Maunakea airglow | International Gemini Observatory/NOIRLab/NSF/AURA/P. Horálek (Institute of Physics in Opava) | CC BY 4.0 | 8K/4K/2K | https://commons.wikimedia.org/wiki/File:Gemini_North_Under_a_Blanket_of_Airglow_360_Panorama_(iotw2509b).jpg |
| hawaii | Maunakea dusk | International Gemini Observatory/NOIRLab/NSF/AURA/P. Horálek | CC BY 4.0 | 8K/4K/2K | https://commons.wikimedia.org/wiki/File:Gemini_North_Sunset_360_Panorama_(2022_04_01_Gemini_North_360_Sunset3-CC).jpg |
| hawaii | Maunakea sunset | International Gemini Observatory/NOIRLab/NSF/AURA/ T. Slovinský | CC BY 4.0 | 8K/4K/2K | https://commons.wikimedia.org/wiki/File:Maunakea_Sunset_360_Panorama_(Gemini_North_360_Sunset4-CC).jpg |
| hawaii | Maunakea sunset above the clouds | International Gemini Observatory/NOIRLab/NSF/AURA/P. Horálek | CC BY 4.0 | 8K/4K/2K | https://commons.wikimedia.org/wiki/File:Gemini_North_Sunset_360_Panorama_(2022_04_01_Gemini_North_360_Sunset2-CC).jpg |
| hawaii | Milky Way over Maunakea | International Gemini Observatory/NOIRLab/NSF/AURA/ T. Slovinský | CC BY 4.0 | 8K/4K/2K | https://commons.wikimedia.org/wiki/File:Milky_Way_Over_Maunakea_360_Panorama_(MaunaKea_GeminiNorth_MW360Pano-CC).jpg |
| lake | Lakeside sunrise | Greg Zaal, Jarod Guest | CC0 1.0 | 8K/4K/2K | https://polyhaven.com/a/lakeside_sunrise |
| mountains | Alps Field | Andreas Mischok | CC0 | 8K/4K/2K | https://polyhaven.com/a/alps_field |
| mountains | Autumn Hill View | Grzegorz Wronkowski | CC0 | 8K/4K/2K | https://polyhaven.com/a/autumn_hill_view |
| mountains | Bergen | Greg Zaal | CC0 | 8K/4K/2K | https://polyhaven.com/a/bergen |
| mountains | Cayley Lookout | Greg Zaal | CC0 | 8K/4K/2K | https://polyhaven.com/a/cayley_lookout |
| mountains | Solitary mountain | Dimitrios Savva, Jarod Guest | CC0 1.0 | 8K/4K/2K | https://polyhaven.com/a/drakensberg_solitary_mountain |
| mountains | Table Mountain 1 (Pure Sky) | Greg Zaal, Jarod Guest | CC0 | 8K/4K/2K | https://polyhaven.com/a/table_mountain_1_puresky |
| night | Clarens Night 02 | Dimitrios Savva, Jarod Guest | CC0 | 8K/4K/2K | https://polyhaven.com/a/clarens_night_02 |
| night | Kloppenheim 02 (Pure Sky) | Greg Zaal, Jarod Guest | CC0 | 8K/4K/2K | https://polyhaven.com/a/kloppenheim_02_puresky |
| night | Moonlit field | Greg Zaal | CC0 1.0 | 8K/4K/2K | https://polyhaven.com/a/moonlit_golf |
| night | Night sky | Greg Zaal | CC0 1.0 | 8K/4K/2K | https://polyhaven.com/a/rogland_clear_night |
| night | Rogland Moonlit Night | Greg Zaal | CC0 | 8K/4K/2K | https://polyhaven.com/a/rogland_moonlit_night |
| ocean | Beach | Andreas Mischok | CC0 1.0 | 8K/4K/2K | https://polyhaven.com/a/spiaggia_di_mondello |
| ocean | Blouberg Sunrise 1 | Greg Zaal, Rico Cilliers | CC0 | 8K/4K/2K | https://polyhaven.com/a/blouberg_sunrise_1 |
| ocean | Blouberg Sunrise 2 | Greg Zaal, Rico Cilliers | CC0 | 8K/4K/2K | https://polyhaven.com/a/blouberg_sunrise_2 |
| ocean | Blue Lagoon | Greg Zaal | CC0 | 8K/4K/2K | https://polyhaven.com/a/blue_lagoon |
| ocean | Fish Hoek Beach | Greg Zaal, Rico Cilliers | CC0 | 8K/4K/2K | https://polyhaven.com/a/fish_hoek_beach |
| ocean | Ocean waves | Greg Zaal | CC0 1.0 | 8K/4K/2K | https://polyhaven.com/a/umhlanga_sunrise |
| ocean | The Sky Is On Fire | Greg Zaal, Rico Cilliers | CC0 | 8K/4K/2K | https://polyhaven.com/a/the_sky_is_on_fire |
| ocean | Tropical beach | Greg Zaal, Rico Cilliers | CC0 1.0 | 8K/4K/2K | https://polyhaven.com/a/secluded_beach |
| ocean | White Cliff Top | Greg Zaal, Rico Cilliers | CC0 | 8K/4K/2K | https://polyhaven.com/a/white_cliff_top |
| snow | Snowy lake | Andreas Mischok | CC0 1.0 | 8K/4K/2K | https://polyhaven.com/a/lago_disola |
| sunset | Bambanani Sunset | Dimitrios Savva, Jarod Guest | CC0 | 8K/4K/2K | https://polyhaven.com/a/bambanani_sunset |
| sunset | Belfast Sunset | Dimitrios Savva, Greg Zaal | CC0 | 8K/4K/2K | https://polyhaven.com/a/belfast_sunset |
| sunset | Camdeboo Road | Dario Barresi, Jarod Guest | CC0 | 8K/4K/2K | https://polyhaven.com/a/camdeboo_road |
| sunset | Sunset | Greg Zaal, Jarod Guest | CC0 1.0 | 8K/4K/2K | https://polyhaven.com/a/qwantani_sunset |
| sunset | Venice Sunset | Greg Zaal | CC0 | 8K/4K/2K | https://polyhaven.com/a/venice_sunset |
| underwater | Heron Island reef | © Underwater Earth / XL Catlin Seaview Survey / Christophe Bailhache | CC BY-SA 3.0 | 8K/4K/2K | https://commons.wikimedia.org/wiki/File:Heron_Bommie_SVII.jpg |
| underwater | Lady Elliot Island reef | © Underwater Earth / XL Catlin Seaview Survey / Aaron Spence | CC BY-SA 3.0 | 8K/4K/2K | https://commons.wikimedia.org/wiki/File:Lady_Elliot_Island_SVII.jpg |
| universe | Gaia all-sky star map | ESA/Gaia/DPAC; CC BY-SA 3.0 IGO Acknowledgement: A. Moitinho | CC BY 4.0 | 8K/4K/2K | https://commons.wikimedia.org/wiki/File:Gaia_Visible_Light_All_Sky_(AllSky-18_Optical330-1100nm-Gaia-DR3-StarMap).jpg |
| universe | Gaia colour of the sky | European Space Agency | CC BY-SA 3.0 igo | 8K/4K/2K | https://commons.wikimedia.org/wiki/File:The_colour_of_the_sky_from_Gaia%E2%80%99s_Early_Data_Release_3_ESA22358060.jpg |
| universe | Milky Way | ESO/S. Brunier | CC BY 4.0 | 4K/2K | https://commons.wikimedia.org/wiki/File:ESO_-_Milky_Way.jpg |
| universe | Milky Way at Chajnantor | Y. Beletsky (LCO)/ESO | CC BY 4.0 | 8K/4K/2K | https://commons.wikimedia.org/wiki/File:Milky_Way_at_Chajnantor_(uhd-9428-panorama-eq-extended).jpg |
| universe | Milky Way at Paranal | Y. Beletsky (LCO)/ESO | CC BY 4.0 | 8K/4K/2K | https://commons.wikimedia.org/wiki/File:Milky_Way_at_the_Residencia_(uhd-9180-panorama-eq-extended).jpg |
| universe | Milky Way over Cerro Pachon | Rubin Observatory/NOIRLab/NSF/AURA/P. Horálek | CC BY 4.0 | 8K/4K/2K | https://commons.wikimedia.org/wiki/File:Milky_Way_Above_Cerro_Pach%C3%B3n_360_Panorama_(20220408PachonFPano35mmRubinAuxi_MW_EQ-001-CC-002).jpg |
| universe | Moon and Milky Way over Rubin | RubinObs/NOIRLab/SLAC/NSF/DOE/AURA//P. Horálek (Institute of Physics in Opava) | CC BY 4.0 | 8K/4K/2K | https://commons.wikimedia.org/wiki/File:Rubin,_the_Moon,_and_the_Milky_Way_(360-Panorama)_(iotw2546b).jpg |

66 pictures, 13 themes, 1739 MB of DDS files.
