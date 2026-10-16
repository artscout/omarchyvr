//! `~/.config/pimax/anchor.toml`: the tag and the pose it had in the app world at calibration time.
//!
//! ```toml
//! [tag]
//! family = "tag36h11"
//! id = 0
//! size_m = 0.15
//! [reference]            # world_from_tag in the app world (after any offset that was active), quat x y z w
//! pos = [0.0, 0.0, 0.0]
//! quat = [0.0, 0.0, 0.0, 1.0]
//! saved = "unix 1760000000"
//! [[window]]             # optional: window poses relative to the tag (tag_from_window)
//! name = "terminal"
//! pos = [..]
//! quat = [..]
//! ```
use pimax_cam::geom::Pose;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct WindowRel {
    pub name: String,
    pub tag_from_window: Pose,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Anchor {
    pub id: u32,
    pub size_m: f64,
    pub reference: Pose,
    pub saved: String,
    pub windows: Vec<WindowRel>,
}

pub fn default_path() -> PathBuf {
    std::env::var_os("PIMAX_ANCHOR_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config/pimax/anchor.toml")
        })
}

fn arr(p: &Pose) -> (String, String) {
    let (t, q) = crate::world::pq(p);
    (
        format!("[{:.6}, {:.6}, {:.6}]", t[0], t[1], t[2]),
        format!("[{:.7}, {:.7}, {:.7}, {:.7}]", q[0], q[1], q[2], q[3]),
    )
}

impl Anchor {
    pub fn to_toml(&self) -> String {
        let (p, q) = arr(&self.reference);
        let mut s = format!(
            "# written by `pimax-anchor calibrate`\n[tag]\nfamily = \"tag36h11\"\nid = {}\nsize_m = {}\n\n[reference]\npos = {p}\nquat = {q}\nsaved = \"{}\"\n",
            self.id, self.size_m, self.saved
        );
        for w in &self.windows {
            let (p, q) = arr(&w.tag_from_window);
            s += &format!("\n[[window]]\nname = \"{}\"\npos = {p}\nquat = {q}\n", w.name);
        }
        s
    }

    pub fn parse(text: &str) -> Result<Anchor, String> {
        let mut sect = String::new();
        let (mut id, mut size) = (None, None);
        let (mut rpos, mut rquat, mut saved) = (None, None, String::new());
        let mut windows: Vec<(String, Option<[f64; 3]>, Option<[f64; 4]>)> = Vec::new();
        for line in text.lines() {
            let l = line.split('#').next().unwrap().trim();
            if l.is_empty() {
                continue;
            }
            if l.starts_with('[') {
                sect = l.trim_matches(|c| c == '[' || c == ']').to_string();
                if sect == "window" {
                    windows.push((String::new(), None, None));
                }
                continue;
            }
            let (k, v) = l.split_once('=').ok_or(format!("bad line: {l}"))?;
            let (k, v) = (k.trim(), v.trim());
            let nums = || -> Result<Vec<f64>, String> {
                v.trim_matches(|c| c == '[' || c == ']')
                    .split(',')
                    .map(|x| x.trim().parse::<f64>().map_err(|e| format!("{k}: {e}")))
                    .collect()
            };
            let s = v.trim_matches('"').to_string();
            match (sect.as_str(), k) {
                ("tag", "id") => id = Some(v.parse::<u32>().map_err(|e| e.to_string())?),
                ("tag", "size_m") => size = Some(v.parse::<f64>().map_err(|e| e.to_string())?),
                ("reference", "pos") => rpos = nums()?.try_into().ok(),
                ("reference", "quat") => rquat = nums()?.try_into().ok(),
                ("reference", "saved") => saved = s,
                ("window", "name") => windows.last_mut().unwrap().0 = s,
                ("window", "pos") => windows.last_mut().unwrap().1 = nums()?.try_into().ok(),
                ("window", "quat") => windows.last_mut().unwrap().2 = nums()?.try_into().ok(),
                _ => {}
            }
        }
        Ok(Anchor {
            id: id.ok_or("tag.id missing")?,
            size_m: size.ok_or("tag.size_m missing")?,
            reference: crate::world::from_pq(rpos.ok_or("reference.pos missing")?, rquat.ok_or("reference.quat missing")?),
            saved,
            windows: windows
                .into_iter()
                .filter_map(|(n, p, q)| Some(WindowRel { name: n, tag_from_window: crate::world::from_pq(p?, q?) }))
                .collect(),
        })
    }

    pub fn load() -> Result<Anchor, String> {
        let p = default_path();
        Anchor::parse(&std::fs::read_to_string(&p).map_err(|e| format!("{}: {e} (run `pimax-anchor calibrate` first)", p.display()))?)
    }

    pub fn save(&self) -> std::io::Result<PathBuf> {
        let p = default_path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = p.with_extension("toml.tmp");
        std::fs::write(&tmp, self.to_toml())?;
        std::fs::rename(&tmp, &p)?;
        Ok(p)
    }
}
