//! Resolve configured trees and their nested mounts to filesystem-relative paths on Linux.

use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    os::unix::ffi::OsStringExt,
    path::{Path, PathBuf},
};

use super::paths_overlap;

pub(super) struct MountTable<'a> {
    root: Mount<'a>,
    children: HashMap<&'a [u8], Vec<Mount<'a>>>,
}

struct Mount<'a> {
    id: &'a [u8],
    device: &'a [u8],
    root: Option<PathBuf>,
    point: PathBuf,
}

pub(super) struct BackingPath<'a> {
    device: &'a [u8],
    path: PathBuf,
}

impl BackingPath<'_> {
    pub(super) fn overlaps(&self, other: &Self) -> bool {
        self.device == other.device && paths_overlap(&self.path, &other.path)
    }
}

impl<'a> MountTable<'a> {
    pub(super) fn parse(input: &'a [u8]) -> Option<Self> {
        let mut mounts = Vec::new();
        let mut ids = HashSet::new();
        for line in input
            .split(|&byte| byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            let mut fields = line.split(|&byte| byte == b' ');
            let id = fields.next()?;
            let parent = fields.next()?;
            let device = fields.next()?;
            let root = fields.next()?;
            // Namespace mounts can have opaque roots such as mnt:[4026537000].
            let root = if root.starts_with(b"/") {
                Some(mount_path(root)?)
            } else {
                None
            };
            let point = mount_path(fields.next()?)?;
            if !ids.insert(id) {
                return None;
            }
            mounts.push((
                parent,
                Mount {
                    id,
                    device,
                    root,
                    point,
                },
            ));
        }
        let root_index = mounts.iter().position(|(parent, mount)| {
            mount.point == Path::new("/") && (*parent == mount.id || !ids.contains(parent))
        })?;
        let (_, root) = mounts.swap_remove(root_index);
        let mut children = HashMap::<_, Vec<_>>::new();
        for (parent, mount) in mounts {
            children.entry(parent).or_default().push(mount);
        }
        Some(Self { root, children })
    }

    pub(super) fn backing_paths(&self, path: &Path) -> Option<Vec<BackingPath<'a>>> {
        let mut pending = vec![(&self.root, path)];
        let mut paths = Vec::new();
        while let Some((mount, scope)) = pending.pop() {
            let children = self.children.get(mount.id).map_or(&[][..], Vec::as_slice);
            // Follow the covering mount first, including an overmount at the same point.
            if let Some(cover) = children
                .iter()
                .filter(|child| scope.starts_with(&child.point))
                .min_by_key(|child| child.point.as_os_str().len())
            {
                pending.push((cover, scope));
                continue;
            }
            let Some(root) = &mount.root else {
                // An opaque mount may cover a configured path, but a nested namespace
                // file has no filesystem subtree to compare for bind-mount aliases.
                if scope == path {
                    return None;
                }
                continue;
            };
            let relative = scope.strip_prefix(&mount.point).ok()?;
            paths.push(BackingPath {
                device: mount.device,
                path: root.join(relative),
            });
            // A bind mount within the scope may expose a subtree rooted elsewhere.
            pending.extend(children.iter().filter_map(|child| {
                child
                    .point
                    .starts_with(scope)
                    .then_some((child, child.point.as_path()))
            }));
        }
        Some(paths)
    }
}

// mountinfo represents spaces, tabs, newlines and backslashes with octal escapes.
fn mount_path(field: &[u8]) -> Option<PathBuf> {
    let mut segments = field.split(|&byte| byte == b'\\');
    let mut decoded = segments.next()?.to_vec();
    for segment in segments {
        let (escape, rest) = segment.split_at_checked(3)?;
        decoded.push(match escape {
            b"040" => b' ',
            b"011" => b'\t',
            b"012" => b'\n',
            b"134" => b'\\',
            _ => return None,
        });
        decoded.extend_from_slice(rest);
    }
    let path = PathBuf::from(OsString::from_vec(decoded));
    path.is_absolute().then_some(path)
}
