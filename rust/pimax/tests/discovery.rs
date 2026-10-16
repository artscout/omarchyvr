mod common;
use pimax::discovery::{self, Roots};
use std::os::unix::fs::symlink;
use std::path::Path;

fn add(root: &Path, hidraw: &str, vid_pid: &str, iface: &str, bus: &str) {
    let ifdir = root.join(format!("sys/devices/pci/usb1/{bus}/{bus}:1.{iface}"));
    let hid = ifdir.join(format!("0003:{vid_pid}.0001"));
    std::fs::create_dir_all(&hid).unwrap();
    std::fs::write(ifdir.join("bInterfaceNumber"), format!("0{iface}\n")).unwrap();
    std::fs::write(
        hid.join("uevent"),
        format!("DRIVER=hid-generic\nHID_ID=0003:{vid_pid}\nHID_NAME=x\n"),
    )
    .unwrap();
    let cls = root.join("sys/class/hidraw").join(hidraw);
    std::fs::create_dir_all(&cls).unwrap();
    symlink(&hid, cls.join("device")).unwrap();
}

#[test]
fn finds_four_interfaces_and_bridge_in_fake_sysfs() {
    let root = common::tmpdir("sysfs");
    for (n, i) in [(5, "0"), (6, "1"), (7, "2"), (8, "3")] {
        add(&root, &format!("hidraw{n}"), "000034A4:00000044", i, "1-1");
    }
    add(&root, "hidraw2", "000034A4:00000043", "0", "1-2");
    add(&root, "hidraw0", "0000046D:0000C52B", "0", "1-3"); // unrelated mouse
    std::fs::create_dir_all(root.join("sys/class/hidraw/notahid")).unwrap();
    let roots = Roots {
        hidraw_class: root.join("sys/class/hidraw"),
        dev: "/dev".into(),
    };
    let f = discovery::find_interfaces(&roots);
    let names: Vec<_> = f
        .iter()
        .map(|p| p.as_ref().map(|p| p.display().to_string()))
        .collect();
    assert_eq!(
        names,
        [
            Some("/dev/hidraw5".into()),
            Some("/dev/hidraw6".into()),
            Some("/dev/hidraw7".into()),
            Some("/dev/hidraw8".into())
        ]
    );
    assert_eq!(discovery::mask(&f), 0b1111);
    assert_eq!(
        discovery::find_bridge_log(&roots)
            .unwrap()
            .display()
            .to_string(),
        "/dev/hidraw2"
    );
}

#[test]
fn empty_or_missing_sysfs_finds_nothing() {
    let root = common::tmpdir("sysfs-empty");
    let roots = Roots {
        hidraw_class: root.join("nope"),
        dev: "/dev".into(),
    };
    assert_eq!(discovery::mask(&discovery::find_interfaces(&roots)), 0);
    assert!(discovery::find_bridge_log(&roots).is_none());
}

#[test]
fn partial_interfaces_mask() {
    let root = common::tmpdir("sysfs-partial");
    add(&root, "hidraw3", "000034A4:00000044", "0", "1-1");
    add(&root, "hidraw4", "000034A4:00000044", "2", "1-1");
    let roots = Roots {
        hidraw_class: root.join("sys/class/hidraw"),
        dev: root.join("dev"),
    };
    let f = discovery::find_interfaces(&roots);
    assert_eq!(discovery::mask(&f), 0b0101);
    assert_eq!(f[2].as_ref().unwrap(), &root.join("dev/hidraw4"));
}
