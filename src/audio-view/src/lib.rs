#![allow(clippy::not_unsafe_ptr_arg_deref)]

pub mod tags;

use ic_plugin_api::{
    check_host, needs_up_to, HostCheck, IcBytes, IcFsSource, IcHost, IcViewVTable, IcViewerVTable,
    IC_ABI_VERSION, IC_ERR_HOST_TOO_OLD, IC_ERR_HOST_UNKNOWN, IC_ERR_INIT_FAILED, IC_HOST_CONSOLE,
    IC_OK, IC_OPEN_READ, IC_SEEK_SET,
};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../version.rs"));

ic_plugin_api::declare_about!(
    "ic-audio-view",
    "Music",
    plugins_version!(),
    "Plays a track, with the rest of the folder as a playlist"
);

pub const ID: &str = "audio-view";
pub const EXTENSIONS: &str = ".mp3,.flac,.ogg,.oga,.opus,.wav,.m4a,.aac,.wma,.aiff,.mp4a";

const NO_SLEEVE: &[u8] = include_bytes!("../assets/music.svg");

pub const SLEEVE: u32 = 120;

static HOST: AtomicUsize = AtomicUsize::new(0);

fn host() -> *const IcHost {
    HOST.load(Ordering::Relaxed) as *const IcHost
}

fn tag_of(source: IcFsSource, name: &str) -> tags::Tags {
    let host = host();
    let Ok(named) = CString::new(name) else {
        return tags::Tags::default();
    };
    if host.is_null() {
        return tags::Tags::default();
    }
    let stream = unsafe { ((*host).fs_open)(source, named.as_ptr(), IC_OPEN_READ) };
    if stream.is_null() {
        return tags::Tags::default();
    }
    let mut head = [0u8; 10];
    let read = unsafe { ((*host).fs_read)(stream, head.as_mut_ptr(), head.len() as u64) };
    let held = if read == head.len() as i64 && head.starts_with(b"ID3") {
        let len = head[6..10]
            .iter()
            .fold(0usize, |held, byte| (held << 7) | (*byte as usize & 0x7f));
        let mut whole = vec![0u8; head.len() + len];
        whole[..head.len()].copy_from_slice(&head);
        unsafe { ((*host).fs_seek)(stream, head.len() as i64, IC_SEEK_SET) };
        let mut filled = head.len();
        while filled < whole.len() {
            let read = unsafe {
                ((*host).fs_read)(
                    stream,
                    whole[filled..].as_mut_ptr(),
                    (whole.len() - filled) as u64,
                )
            };
            if read <= 0 {
                break;
            }
            filled += read as usize;
        }
        whole.truncate(filled);
        whole
    } else {
        Vec::new()
    };
    unsafe { ((*host).fs_close)(stream) };
    tags::read_id3(&held)
}

struct Showing {
    source: IcFsSource,
    tracks: Vec<String>,
    at: usize,
    said: tags::Tags,
    playing: bool,
}

thread_local! {
    static SHOWING: RefCell<BTreeMap<u64, Showing>> = const { RefCell::new(BTreeMap::new()) };
    static DOCUMENT: RefCell<String> = const { RefCell::new(String::new()) };
    static ANSWER: RefCell<String> = const { RefCell::new(String::new()) };
}

pub fn is_a_track(name: &str) -> bool {
    let lowered = name.to_lowercase();
    EXTENSIONS
        .split(',')
        .any(|claim| lowered.ends_with(claim.trim()))
}

fn tracks_beside(source: IcFsSource) -> Vec<String> {
    let host = host();
    if host.is_null() {
        return Vec::new();
    }
    let Ok(here) = CString::new("/") else {
        return Vec::new();
    };
    let listing = unsafe { ((*host).fs_list)(source, here.as_ptr()) };
    let mut found: Vec<String> = listing
        .as_slice()
        .iter()
        .filter(|entry| entry.is_dir == 0)
        .map(|entry| entry.name_string())
        .filter(|name| is_a_track(name))
        .collect();
    found.sort_by_key(|name| name.to_lowercase());
    found
}

extern "C" fn viewer_open(
    instance: u64,
    source: IcFsSource,
    path: *const c_char,
    _user_data: *mut c_void,
) -> c_int {
    if path.is_null() {
        return IC_ERR_INIT_FAILED;
    }
    let opened = unsafe { CStr::from_ptr(path) }
        .to_string_lossy()
        .trim_matches('/')
        .to_string();
    let mut tracks = tracks_beside(source);
    if !tracks.iter().any(|name| *name == opened) {
        tracks = vec![opened.clone()];
    }
    let at = tracks.iter().position(|name| *name == opened).unwrap_or(0);
    let said = tag_of(source, &opened);
    SHOWING.with(|held| {
        held.borrow_mut().insert(
            instance,
            Showing {
                source,
                tracks,
                at,
                said,
                playing: true,
            },
        )
    });
    IC_OK
}

extern "C" fn viewer_closed(instance: u64, _user_data: *mut c_void) {
    SHOWING.with(|held| held.borrow_mut().remove(&instance));
}

pub fn document_for(tracks: &[String], at: usize, said: &tags::Tags, playing: bool) -> String {
    let now = tracks.get(at).cloned().unwrap_or_default();
    let headline = said.headline(&now);
    let sleeve = serde_json::json!({
        "t": "image",
        "id": "cover",
        "src": "part:cover",
        "fit": "contain",
        "height": SLEEVE
    });

    let rows: Vec<serde_json::Value> = tracks
        .iter()
        .enumerate()
        .map(|(index, name)| {
            serde_json::json!({
                "t": "button",
                "id": format!("track-{index}"),
                "role": if index == at { "row_selected" } else { "row" },
                "label": { "literal": if index == at {
                    format!("\u{25b6}  {name}")
                } else {
                    format!("     {name}")
                } },
                "sensitive": { "truthy": format!("data.pick-{index}") },
                "intent": { "do": "emit", "node": format!("track-{index}") }
            })
        })
        .collect();

    let mut data = serde_json::Map::new();
    data.insert("of".to_string(), serde_json::json!(tracks.len()));
    data.insert("at".to_string(), serde_json::json!(at + 1));
    data.insert("can_back".to_string(), serde_json::json!(at > 0));
    data.insert(
        "can_go_on".to_string(),
        serde_json::json!(at + 1 < tracks.len()),
    );
    for index in 0..tracks.len() {
        data.insert(format!("pick-{index}"), serde_json::json!(index != at));
    }

    let mut children: Vec<serde_json::Value> = Vec::new();
    children.push(serde_json::json!({
        "t": "row",
        "spacing": 12,
        "children": [
            sleeve,
            {
                "t": "column",
                "id": "about",
                "spacing": 6,
                "weight": 1,
                "children": [
                    { "t": "text", "id": "playing", "role": "title1", "wrap": true,
                      "text": { "literal": headline } },
                    { "t": "text", "id": "album", "role": "dim",
                      "text": { "literal": said.album.clone().unwrap_or_default() } },
                    { "t": "media", "id": "sound", "media": "audio",
                      "src": format!("file:{now}"), "autoplay": playing },
                    {
                        "t": "row",
                        "spacing": 8,
                        "children": [
                            { "t": "button", "id": "back", "label": { "literal": "\u{25c0}" },
                              "accel": "Left",
                              "sensitive": { "truthy": "data.can_back" },
                              "intent": { "do": "emit", "node": "back" } },
                            { "t": "button", "id": "on", "label": { "literal": "\u{25b6}" },
                              "accel": "Right",
                              "sensitive": { "truthy": "data.can_go_on" },
                              "intent": { "do": "emit", "node": "on" } },
                            { "t": "text", "id": "where", "role": "dim", "weight": 1,
                              "text": { "literal": format!("{} / {}", at + 1, tracks.len()) } }
                        ]
                    }
                ]
            }
        ]
    }));
    children.push(serde_json::json!({ "t": "separator" }));
    children.push(serde_json::json!({
        "t": "column", "id": "playlist", "scroll": "vertical", "weight": 1,
        "children": rows
    }));

    serde_json::json!({
        "schema": 1,
        "data": data,
        "fields": [],
        "form": {
            "t": "view",
            "surface": "window",
            "spacing": 10,
            "padding": 12,
            "children": children
        }
    })
    .to_string()
}

fn asking_about(raw: *const u8, len: u64) -> u64 {
    if raw.is_null() || len == 0 {
        return 0;
    }
    let held = String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(raw, len as usize) });
    serde_json::from_str::<serde_json::Value>(&held)
        .ok()
        .and_then(|held| held.get("instance").and_then(|at| at.as_u64()))
        .unwrap_or(0)
}

extern "C" fn viewer_describe(ctx: *const u8, len: u64, _user_data: *mut c_void) -> IcBytes {
    let asked = asking_about(ctx, len);
    let drawn = SHOWING.with(|held| {
        let held = held.borrow();
        let showing = held.get(&asked)?;
        Some(document_for(
            &showing.tracks,
            showing.at,
            &showing.said,
            showing.playing,
        ))
    });
    let Some(drawn) = drawn else {
        return IcBytes::EMPTY;
    };
    DOCUMENT.with(|held| {
        let mut held = held.borrow_mut();
        *held = drawn;
        IcBytes {
            data: held.as_ptr(),
            len: held.len() as u64,
        }
    })
}

fn wanted(node: &str, showing: &Showing) -> Option<usize> {
    match node {
        "back" => (showing.at > 0).then(|| showing.at - 1),
        "on" => (showing.at + 1 < showing.tracks.len()).then_some(showing.at + 1),
        named => {
            let picked: usize = named.strip_prefix("track-")?.parse().ok()?;
            (picked < showing.tracks.len() && picked != showing.at).then_some(picked)
        }
    }
}

extern "C" fn viewer_event(raw: *const u8, len: u64, _user_data: *mut c_void) -> IcBytes {
    let asked = asking_about(raw, len);
    let event: serde_json::Value = if raw.is_null() || len == 0 {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(unsafe { std::slice::from_raw_parts(raw, len as usize) })
            .unwrap_or(serde_json::Value::Null)
    };
    let node = event
        .get("node")
        .and_then(|node| node.as_str())
        .unwrap_or("");
    let moved = SHOWING.with(|held| {
        let mut held = held.borrow_mut();
        let Some(showing) = held.get_mut(&asked) else {
            return false;
        };
        let Some(next) = wanted(node, showing) else {
            return false;
        };
        showing.at = next;
        showing.said = tag_of(showing.source, &showing.tracks[next]);
        showing.playing = true;
        true
    });
    answer_with(if moved {
        r#"{"redescribe":true}"#
    } else {
        "{}"
    })
}

fn answer_with(text: &str) -> IcBytes {
    ANSWER.with(|held| {
        let mut held = held.borrow_mut();
        *held = text.to_string();
        IcBytes {
            data: held.as_ptr(),
            len: held.len() as u64,
        }
    })
}

pub fn view_vtable() -> IcViewVTable {
    IcViewVTable {
        struct_size: std::mem::size_of::<IcViewVTable>() as u32,
        describe: viewer_describe,
        on_event: Some(viewer_event),
        closed: None,
    }
}

extern "C" fn viewer_content(
    instance: u64,
    name: *const c_char,
    _user_data: *mut c_void,
) -> IcBytes {
    if name.is_null() {
        return IcBytes::EMPTY;
    }
    let asked = unsafe { CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned();
    if asked != "cover" {
        return IcBytes::EMPTY;
    }
    SHOWING.with(|held| {
        let held = held.borrow();
        let Some(showing) = held.get(&instance) else {
            return IcBytes::EMPTY;
        };
        match showing.said.cover.as_ref() {
            Some(cover) => IcBytes {
                data: cover.as_ptr(),
                len: cover.len() as u64,
            },
            None => IcBytes {
                data: NO_SLEEVE.as_ptr(),
                len: NO_SLEEVE.len() as u64,
            },
        }
    })
}

pub fn viewer_vtable(view: *const IcViewVTable) -> IcViewerVTable {
    IcViewerVTable {
        struct_size: std::mem::size_of::<IcViewerVTable>() as u32,
        view,
        open: viewer_open,
        closed: Some(viewer_closed),
        content: Some(viewer_content),
        closing: None,
        canvas_ready: None,
        canvas_draw: None,
        canvas_gone: None,
    }
}

fn in_console(kind: *const c_char) -> bool {
    !kind.is_null() && unsafe { CStr::from_ptr(kind) }.to_bytes() == IC_HOST_CONSOLE.as_bytes()
}

#[cfg_attr(feature = "export-abi", no_mangle)]
pub extern "C" fn ic_plugin_init(host: *const IcHost, kind: *const c_char) -> c_int {
    if in_console(kind) {
        return ic_plugin_api::IC_ERR_NOT_THIS_HOST;
    }
    match check_host(
        host,
        IC_ABI_VERSION,
        needs_up_to(std::mem::offset_of!(IcHost, register_viewer)),
    ) {
        HostCheck::Ok => {}
        HostCheck::WrongMagic => return IC_ERR_HOST_UNKNOWN,
        HostCheck::TooOld { .. } | HostCheck::Truncated { .. } => return IC_ERR_HOST_TOO_OLD,
    }
    HOST.store(host as usize, Ordering::Relaxed);
    let (Ok(id), Ok(extensions)) = (CString::new(ID), CString::new(EXTENSIONS)) else {
        return IC_ERR_INIT_FAILED;
    };
    let window = view_vtable();
    let viewer = viewer_vtable(&window);
    unsafe {
        ((*host).register_viewer)(
            id.as_ptr(),
            extensions.as_ptr(),
            0,
            &viewer,
            std::ptr::null_mut(),
        )
    }
}

#[cfg_attr(feature = "export-abi", no_mangle)]
pub extern "C" fn ic_plugin_shutdown() {
    SHOWING.with(|held| held.borrow_mut().clear());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder() -> Vec<String> {
        ["one.mp3", "two.flac", "three.ogg"]
            .iter()
            .map(|name| (*name).to_string())
            .collect()
    }

    #[test]
    fn the_sleeve_is_handed_over_by_name() {
        SHOWING.with(|held| {
            held.borrow_mut().insert(
                8,
                Showing {
                    source: std::ptr::null_mut(),
                    tracks: folder(),
                    at: 0,
                    said: tags::Tags {
                        cover: Some(b"sleeve".to_vec()),
                        ..tags::Tags::default()
                    },
                    playing: false,
                },
            )
        });
        let asked = CString::new("cover").expect("a name");
        let answered = viewer_content(8, asked.as_ptr(), std::ptr::null_mut());
        assert_eq!(
            unsafe { std::slice::from_raw_parts(answered.data, answered.len as usize) },
            b"sleeve"
        );
        let nothing = CString::new("something-else").expect("a name");
        assert!(viewer_content(8, nothing.as_ptr(), std::ptr::null_mut())
            .data
            .is_null());

        SHOWING.with(|held| {
            held.borrow_mut().get_mut(&8).expect("a window").said = tags::Tags::default()
        });
        let answered = viewer_content(8, asked.as_ptr(), std::ptr::null_mut());
        assert_eq!(
            unsafe { std::slice::from_raw_parts(answered.data, answered.len as usize) },
            NO_SLEEVE,
            "the picture that stands in for a sleeve"
        );
        assert!(viewer_content(404, asked.as_ptr(), std::ptr::null_mut())
            .data
            .is_null());
        viewer_closed(8, std::ptr::null_mut());
    }

    #[test]
    fn only_what_this_plugin_offered_to_play_is_in_the_playlist() {
        assert!(
            is_a_track("Holiday.MP3"),
            "the name is read whatever its case"
        );
        assert!(is_a_track("a.flac"));
        assert!(!is_a_track("notes.txt"));
        assert!(!is_a_track("mp3"), "an extension is not a name");
    }

    #[test]
    fn pressing_moves_through_the_folder() {
        SHOWING.with(|held| {
            held.borrow_mut().insert(
                3,
                Showing {
                    source: std::ptr::null_mut(),
                    tracks: folder(),
                    at: 0,
                    said: tags::Tags::default(),
                    playing: false,
                },
            )
        });

        assert_eq!(pressed(3, "back"), "{}", "the first track stays put");
        assert_eq!(pressed(3, "on"), r#"{"redescribe":true}"#);
        assert!(described(3).contains("file:two.flac"));
        assert!(
            described(3).contains("\"autoplay\":true"),
            "a track the user moved to plays"
        );

        assert_eq!(pressed(3, "track-2"), r#"{"redescribe":true}"#);
        assert!(described(3).contains("file:three.ogg"));
        assert_eq!(pressed(3, "track-2"), "{}", "the one playing is not a move");
        assert_eq!(pressed(3, "on"), "{}", "and the last track stays put");

        assert_eq!(pressed(3, "track-9"), "{}", "a row that is not there");
        assert_eq!(pressed(99, "on"), "{}", "a window nobody opened");
        viewer_closed(3, std::ptr::null_mut());
        assert!(described(3).is_empty());
    }

    fn pressed(instance: u64, node: &str) -> String {
        let event = serde_json::json!({
            "type": "activate",
            "node": node,
            "values": {},
            "instance": instance
        })
        .to_string();
        answered(viewer_event(
            event.as_ptr(),
            event.len() as u64,
            std::ptr::null_mut(),
        ))
    }

    fn described(instance: u64) -> String {
        let context = format!(r#"{{"host":{{"kind":"gtk"}},"instance":{instance}}}"#);
        answered(viewer_describe(
            context.as_ptr(),
            context.len() as u64,
            std::ptr::null_mut(),
        ))
    }

    fn answered(bytes: IcBytes) -> String {
        if bytes.data.is_null() {
            return String::new();
        }
        String::from_utf8_lossy(unsafe {
            std::slice::from_raw_parts(bytes.data, bytes.len as usize)
        })
        .into_owned()
    }

    #[test]
    fn a_terminal_has_neither_player_nor_sound() {
        let host = ic_plugin_api::testing::silent_host();
        let console = CString::new(IC_HOST_CONSOLE).expect("a kind");
        assert_ne!(ic_plugin_init(&host, console.as_ptr()), IC_OK);
    }
}

#[cfg(test)]
mod reading_the_folder {
    use super::*;

    thread_local! {
        static NAMES: RefCell<Vec<CString>> = const { RefCell::new(Vec::new()) };
        static ROWS: RefCell<Vec<ic_plugin_api::IcDirEntry>> = const { RefCell::new(Vec::new()) };
    }

    extern "C" fn lists(_: IcFsSource, _: *const c_char) -> ic_plugin_api::IcListing {
        const HELD: [(&str, c_int); 5] = [
            ("02 second.mp3", 0),
            ("01 first.flac", 0),
            ("notes.txt", 0),
            ("03 third.ogg", 0),
            ("covers", 1),
        ];
        NAMES.with(|names| {
            let mut names = names.borrow_mut();
            *names = HELD
                .iter()
                .map(|(name, _)| CString::new(*name).expect("a name"))
                .collect();
            ROWS.with(|rows| {
                let mut rows = rows.borrow_mut();
                *rows = HELD
                    .iter()
                    .zip(names.iter())
                    .map(|((_, is_dir), named)| ic_plugin_api::IcDirEntry {
                        name: named.as_ptr(),
                        is_dir: *is_dir,
                        size: 0,
                        modified: 0,
                        permissions: 0,
                        has_permissions: 0,
                    })
                    .collect();
                ic_plugin_api::IcListing {
                    items: rows.as_ptr(),
                    count: rows.len() as u32,
                }
            })
        })
    }

    #[test]
    fn opening_one_song_finds_the_others() {
        let mut host = ic_plugin_api::testing::silent_host();
        host.fs_list = lists;
        HOST.store(&host as *const IcHost as usize, Ordering::Relaxed);

        let opened = CString::new("02 second.mp3").expect("a path");
        assert_eq!(
            viewer_open(
                11,
                std::ptr::null_mut(),
                opened.as_ptr(),
                std::ptr::null_mut()
            ),
            IC_OK
        );
        let (tracks, at) = SHOWING.with(|held| {
            let held = held.borrow();
            let showing = held.get(&11).expect("a window");
            (showing.tracks.clone(), showing.at)
        });
        assert_eq!(
            tracks,
            vec![
                "01 first.flac".to_string(),
                "02 second.mp3".to_string(),
                "03 third.ogg".to_string()
            ],
            "three songs, in order, and nothing that is not one"
        );
        assert_eq!(at, 1, "the one that was opened is the one playing");

        viewer_closed(11, std::ptr::null_mut());
        HOST.store(0, Ordering::Relaxed);
    }
}
