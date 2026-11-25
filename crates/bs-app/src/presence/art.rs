//! Иконка игры с машины игрока: порт `gameIcons.ts` и `peIcon.ts` из PR
//! #482 оригинального Backspace (QuddyMusic, cirzear, Z.ai). Иконка
//! загружается на свой инстанс и попадает в активность ссылкой оттуда.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use super::games::Detected;
use super::stores;

/// Сторона иконки, которую отправляем на сервер.
const SIDE: u32 = 256;
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const SIZES: &[&str] = &[
    "512x512", "384x384", "256x256", "192x192", "128x128", "96x96", "64x64", "48x48", "32x32",
];
const SIBLINGS: &[&str] = &[
    ".DirIcon",
    "icon.png",
    "Icon.png",
    "icon_256.png",
    "logo.png",
    "cover.png",
];
/// Программы-«носители»: их .desktop не говорит, какая это игра.
const HOSTS: &[&str] = &[
    "java",
    "javaw",
    "python",
    "python3",
    "wine",
    "wine64",
    "wine-preloader",
    "wine64-preloader",
    "mono",
    "dotnet",
    "node",
    "electron",
    "ruby",
    "perl",
    "lua",
    "luajit",
    "love",
    "sh",
    "bash",
    "dash",
    "zsh",
    "env",
    "flatpak",
    "bwrap",
    "steam",
    "reaper",
    "proton",
    "gamescope",
    "gamemoderun",
    "mangohud",
];

/// Картинка → PNG не больше 256×256.
fn normalized(bytes: &[u8]) -> Option<Vec<u8>> {
    let image = image::load_from_memory(bytes).ok()?;
    let image = if image.width().max(image.height()) > SIDE {
        image.thumbnail(SIDE, SIDE)
    } else {
        image
    };
    let mut out = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some(out)
}

fn read_image(path: &Path) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_FILE_BYTES {
        return None;
    }
    normalized(&crate::files::read(path).ok()?)
}

fn first_image(paths: impl IntoIterator<Item = PathBuf>) -> Option<Vec<u8>> {
    paths.into_iter().find_map(|p| read_image(&p))
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn steam_icon(app_id: &str) -> Option<Vec<u8>> {
    let roots = stores::steam_roots(&home());
    first_image(roots.iter().flat_map(|root| {
        let cache = root.join("appcache/librarycache");
        [
            cache.join(format!("{app_id}_icon.jpg")),
            cache.join(app_id).join("library_600x900.jpg"),
            cache.join(app_id).join("header.jpg"),
            cache.join(format!("{app_id}.jpg")),
        ]
    }))
}

fn themed(icon: &str) -> Vec<PathBuf> {
    if Path::new(icon).is_absolute() {
        return vec![PathBuf::from(icon)];
    }
    let home = home();
    let file = if Path::new(icon)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("png"))
    {
        icon.to_owned()
    } else {
        format!("{icon}.png")
    };
    let roots = [
        PathBuf::from("/usr/share/icons/hicolor"),
        home.join(".local/share/icons/hicolor"),
        PathBuf::from("/var/lib/flatpak/exports/share/icons/hicolor"),
        home.join(".local/share/flatpak/exports/share/icons/hicolor"),
    ];
    let name = file.as_str();
    let mut out: Vec<PathBuf> = roots
        .iter()
        .flat_map(|root| {
            SIZES
                .iter()
                .map(move |size| root.join(size).join("apps").join(name))
        })
        .collect();
    out.push(PathBuf::from("/usr/share/pixmaps").join(&file));
    out
}

/// Программа из строки `Exec=` .desktop (без `env` и `VAR=…`).
pub(crate) fn exec_program(exec: &str) -> String {
    let mut tokens = Vec::new();
    let (mut current, mut quoted) = (String::new(), false);
    for c in exec.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    for token in tokens {
        let assignment = token.split_once('=').is_some_and(|(k, _)| {
            !k.is_empty() && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        });
        if token == "env" || assignment {
            continue;
        }
        return token.rsplit('/').next().unwrap_or(&token).to_lowercase();
    }
    String::new()
}

/// Иконка из .desktop: по программе запуска или по названию.
fn desktop_icon(exe: Option<&str>, name: &str) -> Option<Vec<u8>> {
    let home = home();
    let exe_base = exe
        .map(|e| e.rsplit(['/', '\\']).next().unwrap_or(e).to_lowercase())
        .unwrap_or_default();
    let host = HOSTS.contains(&exe_base.as_str())
        || exe_base.starts_with("python")
        || exe_base.starts_with("ld-linux");
    let name = name.trim().to_lowercase();
    let dirs = [
        home.join(".local/share/applications"),
        PathBuf::from("/usr/share/applications"),
        PathBuf::from("/usr/local/share/applications"),
        PathBuf::from("/var/lib/flatpak/exports/share/applications"),
        home.join(".local/share/flatpak/exports/share/applications"),
    ];
    let mut checked = 0;
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if checked >= 2000 {
                return None;
            }
            if entry.path().extension().is_none_or(|e| e != "desktop") {
                continue;
            }
            checked += 1;
            let Some(text) = entry
                .metadata()
                .ok()
                .filter(|m| m.len() <= 64 * 1024)
                .and_then(|_| crate::files::read_to_string(entry.path()).ok())
            else {
                continue;
            };
            let Some(icon) = stores::desktop_value(&text, "Icon") else {
                continue;
            };
            let exec_matches = !exe_base.is_empty()
                && !host
                && stores::desktop_value(&text, "Exec")
                    .is_some_and(|e| exec_program(&e) == exe_base);
            let name_matches = !name.is_empty()
                && stores::desktop_value(&text, "Name")
                    .is_some_and(|n| n.trim().to_lowercase() == name);
            if (exec_matches || name_matches)
                && let Some(found) = first_image(themed(&icon))
            {
                return Some(found);
            }
        }
    }
    None
}

/// Иконка рядом с исполняемым файлом (AppImage `.DirIcon`, `icon.png`…).
fn sibling_icon(exe: &str) -> Option<Vec<u8>> {
    let path = Path::new(exe);
    let dir = path.parent()?;
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned());
    let mut candidates = Vec::new();
    for base in [Some(dir), dir.parent(), dir.parent().and_then(Path::parent)]
        .into_iter()
        .flatten()
    {
        if let Some(stem) = &stem
            && path.extension().is_some()
        {
            candidates.push(base.join(format!("{stem}.png")));
        }
        candidates.extend(SIBLINGS.iter().map(|s| base.join(s)));
    }
    first_image(candidates)
}

/// Иконка найденной игры (блокирующая: диск, разбор .exe).
pub(crate) fn icon_for(game: &Detected) -> Option<Vec<u8>> {
    if let Some(store) = &game.store {
        if let Some(icon) = store.icon.as_deref().and_then(read_image) {
            return Some(icon);
        }
        if let Some(app) = store.id.strip_prefix("steam-")
            && let Some(icon) = steam_icon(app)
        {
            return Some(icon);
        }
    }
    let info = game.info.as_ref();
    // Игра Wine: значок из её .exe (в рабочем каталоге или по пути из
    // командной строки).
    if let Some(info) = info
        && let Some(program) = info.win_exes.first()
    {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(cwd) = &info.cwd {
            candidates.push(Path::new(cwd).join(program));
        }
        for token in info.cmdline.split(' ') {
            if token.to_lowercase().ends_with(".exe") && token.starts_with('/') {
                candidates.push(PathBuf::from(token));
            }
        }
        if let Some(icon) = candidates.iter().find_map(|p| pe_icon(p)) {
            return Some(icon);
        }
    }
    if let Some(exe) = info.and_then(|i| i.exe.as_deref()) {
        if exe.to_lowercase().ends_with(".exe")
            && let Some(icon) = pe_icon(Path::new(exe))
        {
            return Some(icon);
        }
        if let Some(icon) = sibling_icon(exe) {
            return Some(icon);
        }
    }
    if let Some(dir) = game.store.as_ref().and_then(|s| s.dir.as_ref()) {
        let gog = [dir.join("support/icon.png"), dir.join("goggame.ico")];
        if let Some(icon) = first_image(gog) {
            return Some(icon);
        }
    }
    desktop_icon(info.and_then(|i| i.exe.as_deref()), &game.name)
}

// ─── Значок из PE (.exe) ────────────────────────────────────────────────────

const RT_ICON: u32 = 3;
const RT_GROUP_ICON: u32 = 14;
const MAX_RESOURCE: u32 = 1024 * 1024;

struct Pe {
    file: File,
    size: u64,
    /// (виртуальный адрес, протяжённость, смещение в файле).
    sections: Vec<(u32, u32, u32)>,
    base: u64,
    span: u64,
}

fn read_at(file: &mut File, size: u64, offset: u64, len: usize) -> Option<Vec<u8>> {
    if offset.checked_add(len as u64)? > size {
        return None;
    }
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut buf = vec![0; len];
    file.read_exact(&mut buf).ok()?;
    Some(buf)
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

impl Pe {
    fn open(path: &Path) -> Option<Self> {
        let mut file = File::open(path).ok()?;
        let size = file.metadata().ok()?.len();
        let dos = read_at(&mut file, size, 0, 64)?;
        if u16_at(&dos, 0)? != 0x5a4d {
            return None;
        }
        let pe = u64::from(u32_at(&dos, 0x3c)?);
        let coff = read_at(&mut file, size, pe, 24)?;
        if u32_at(&coff, 0)? != 0x4550 {
            return None;
        }
        let count = usize::from(u16_at(&coff, 6)?);
        let optional_size = usize::from(u16_at(&coff, 20)?);
        if count == 0 || count > 96 {
            return None;
        }
        let optional = read_at(&mut file, size, pe + 24, optional_size)?;
        let directories = match u16_at(&optional, 0)? {
            0x10b => 96,
            0x20b => 112,
            _ => return None,
        };
        let rva = u32_at(&optional, directories + 16)?;
        let span = u32_at(&optional, directories + 20)?;
        if rva == 0 || span == 0 {
            return None;
        }
        let table = read_at(&mut file, size, pe + 24 + optional_size as u64, count * 40)?;
        let sections: Vec<(u32, u32, u32)> = (0..count)
            .filter_map(|i| {
                let at = i * 40;
                Some((
                    u32_at(&table, at + 12)?,
                    u32_at(&table, at + 8)?.max(u32_at(&table, at + 16)?),
                    u32_at(&table, at + 20)?,
                ))
            })
            .collect();
        let mut pe = Self {
            file,
            size,
            sections,
            base: 0,
            span: u64::from(span),
        };
        pe.base = pe.offset(rva)?;
        Some(pe)
    }

    fn offset(&self, rva: u32) -> Option<u64> {
        self.sections
            .iter()
            .find(|(va, span, _)| rva >= *va && rva - va < *span)
            .map(|(va, _, raw)| u64::from(*raw) + u64::from(rva - va))
    }

    /// Записи каталога ресурсов: (id, именованная, цель).
    fn directory(&mut self, offset: u64) -> Option<Vec<(u32, bool, u32)>> {
        if offset + 16 > self.span {
            return None;
        }
        let header = read_at(&mut self.file, self.size, self.base + offset, 16)?;
        let count = usize::from(u16_at(&header, 12)?) + usize::from(u16_at(&header, 14)?);
        if count > 4096 {
            return None;
        }
        let raw = read_at(
            &mut self.file,
            self.size,
            self.base + offset + 16,
            count * 8,
        )?;
        Some(
            (0..count)
                .filter_map(|i| {
                    let name = u32_at(&raw, i * 8)?;
                    Some((
                        name & 0xffff,
                        name & 0x8000_0000 != 0,
                        u32_at(&raw, i * 8 + 4)?,
                    ))
                })
                .collect(),
        )
    }

    fn resource(&mut self, kind: u32, id: Option<u32>) -> Option<Vec<u8>> {
        let mut offset = 0u64;
        for depth in 0..3 {
            let entries = self.directory(offset)?;
            let entry = match (depth, id) {
                (0, _) => entries.iter().find(|(i, named, _)| !named && *i == kind),
                (1, Some(id)) => entries.iter().find(|(i, named, _)| !named && *i == id),
                _ => entries.first(),
            }?;
            let target = entry.2;
            if target & 0x8000_0000 != 0 {
                offset = u64::from(target & 0x7fff_ffff);
                continue;
            }
            let data = read_at(&mut self.file, self.size, self.base + u64::from(target), 16)?;
            let (rva, size) = (u32_at(&data, 0)?, u32_at(&data, 4)?);
            if size == 0 || size > MAX_RESOURCE {
                return None;
            }
            let at = self.offset(rva)?;
            return read_at(&mut self.file, self.size, at, size as usize);
        }
        None
    }
}

/// Значок программы Windows: самая крупная картинка из группы, через
/// декодер ICO (он понимает и PNG, и DIB с маской).
pub(crate) fn pe_icon(path: &Path) -> Option<Vec<u8>> {
    let mut pe = Pe::open(path)?;
    let group = pe.resource(RT_GROUP_ICON, None)?;
    if group.len() < 6 || u16_at(&group, 2)? != 1 {
        return None;
    }
    let count = usize::from(u16_at(&group, 4)?).min(64);
    let mut entries: Vec<(u32, u16, u16, [u8; 8])> = (0..count)
        .filter_map(|i| {
            let at = 6 + i * 14;
            let head: [u8; 8] = group.get(at..at + 8)?.try_into().ok()?;
            let width = if group[at] == 0 {
                256
            } else {
                u32::from(group[at])
            };
            Some((
                width,
                u16_at(&group, at + 6)?,
                u16_at(&group, at + 12)?,
                head,
            ))
        })
        .collect();
    entries.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    for (_, _, id, head) in entries {
        let Some(data) = pe.resource(RT_ICON, Some(u32::from(id))) else {
            continue;
        };
        // Файл ICO с одной картинкой: заголовок, запись каталога, данные.
        let mut ico = vec![0, 0, 1, 0, 1, 0];
        ico.extend_from_slice(&head);
        ico.extend_from_slice(&u32::try_from(data.len()).ok()?.to_le_bytes());
        ico.extend_from_slice(&22u32.to_le_bytes());
        ico.extend_from_slice(&data);
        if let Some(png) = normalized(&ico) {
            return Some(png);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_lines_name_their_program() {
        assert_eq!(
            exec_program("env WINEPREFIX=/x /opt/Game/game.x86_64 %U"),
            "game.x86_64"
        );
        assert_eq!(exec_program("\"/opt/My Game/Run\" --x"), "run");
    }

    /// Минимальный PE с одной иконкой 2×2 (PNG) в ресурсах.
    fn tiny_pe() -> Vec<u8> {
        let mut png = Vec::new();
        image::DynamicImage::new_rgba8(2, 2)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap_or_default();
        let mut file = vec![0u8; 0x400];
        file[0] = b'M';
        file[1] = b'Z';
        file[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        file[0x80..0x84].copy_from_slice(&0x4550u32.to_le_bytes());
        file[0x86..0x88].copy_from_slice(&1u16.to_le_bytes()); // секций
        file[0x94..0x96].copy_from_slice(&0xf0u16.to_le_bytes()); // размер optional
        let optional = 0x98;
        file[optional..optional + 2].copy_from_slice(&0x20bu16.to_le_bytes());
        // Каталог ресурсов: RVA 0x1000, размер 0x200.
        file[optional + 112 + 16..optional + 112 + 20].copy_from_slice(&0x1000u32.to_le_bytes());
        file[optional + 112 + 20..optional + 112 + 24].copy_from_slice(&0x200u32.to_le_bytes());
        let section = optional + 0xf0;
        file[section + 8..section + 12].copy_from_slice(&0x1000u32.to_le_bytes());
        file[section + 12..section + 16].copy_from_slice(&0x1000u32.to_le_bytes());
        file[section + 16..section + 20].copy_from_slice(&0x1000u32.to_le_bytes());
        file[section + 20..section + 24].copy_from_slice(&0x400u32.to_le_bytes());
        let mut res = vec![0u8; 0x200];
        let dir = |res: &mut Vec<u8>, at: usize, entries: &[(u32, u32)]| {
            res[at + 14..at + 16]
                .copy_from_slice(&u16::try_from(entries.len()).unwrap_or(0).to_le_bytes());
            for (i, (id, target)) in entries.iter().enumerate() {
                let e = at + 16 + i * 8;
                res[e..e + 4].copy_from_slice(&id.to_le_bytes());
                res[e + 4..e + 8].copy_from_slice(&target.to_le_bytes());
            }
        };
        // Корень: RT_ICON → 0x30, RT_GROUP_ICON → 0x60.
        dir(&mut res, 0, &[(3, 0x8000_0030), (14, 0x8000_0060)]);
        dir(&mut res, 0x30, &[(1, 0x8000_0090)]);
        dir(&mut res, 0x90, &[(0x409, 0xc0)]);
        dir(&mut res, 0x60, &[(1, 0x8000_00a8)]);
        dir(&mut res, 0xa8, &[(0x409, 0xd0)]);
        let group: Vec<u8> = [0u8, 0, 1, 0, 1, 0, 2, 2, 0, 0, 1, 0, 32, 0]
            .iter()
            .copied()
            .chain(u32::try_from(png.len()).unwrap_or(0).to_le_bytes())
            .chain(1u16.to_le_bytes())
            .collect();
        let data_at = 0x100usize;
        let group_at = data_at + png.len();
        res[0xc0..0xc4]
            .copy_from_slice(&(0x1000 + u32::try_from(data_at).unwrap_or(0)).to_le_bytes());
        res[0xc4..0xc8].copy_from_slice(&u32::try_from(png.len()).unwrap_or(0).to_le_bytes());
        res[0xd0..0xd4]
            .copy_from_slice(&(0x1000 + u32::try_from(group_at).unwrap_or(0)).to_le_bytes());
        res[0xd4..0xd8].copy_from_slice(&u32::try_from(group.len()).unwrap_or(0).to_le_bytes());
        res.resize(group_at + group.len(), 0);
        res[data_at..data_at + png.len()].copy_from_slice(&png);
        res[group_at..].copy_from_slice(&group);
        file.extend_from_slice(&res);
        file
    }

    #[test]
    fn icons_come_out_of_executables() {
        let dir = tempfile::tempdir().unwrap_or_else(|_| unreachable!());
        let path = dir.path().join("Game.exe");
        crate::files::write(&path, tiny_pe()).unwrap_or_default();
        let png = pe_icon(&path).unwrap_or_default();
        let image = image::load_from_memory(&png).map(|i| (i.width(), i.height()));
        assert_eq!(image.ok(), Some((2, 2)));
        crate::files::write(&path, b"MZ not really").unwrap_or_default();
        assert!(pe_icon(&path).is_none());
    }
}
