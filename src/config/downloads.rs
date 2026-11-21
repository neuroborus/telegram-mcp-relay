use super::*;

pub(crate) const CLOUD_DOWNLOAD_BYTES: u64 = 20_000_000;
pub(crate) const LOCAL_DOWNLOAD_BYTES: u64 = 4 * (1 << 30);
const CLOUD_DOWNLOAD_TIMEOUT_MAX: u64 = 600;
const LOCAL_DOWNLOAD_TIMEOUT_MAX: u64 = 3_600;

impl BotApi {
    pub(crate) fn download_ceiling(self) -> u64 {
        match self {
            Self::Cloud => CLOUD_DOWNLOAD_BYTES,
            Self::Local => LOCAL_DOWNLOAD_BYTES,
        }
    }

    pub(crate) fn download_timeout_max(self) -> u64 {
        match self {
            Self::Cloud => CLOUD_DOWNLOAD_TIMEOUT_MAX,
            Self::Local => LOCAL_DOWNLOAD_TIMEOUT_MAX,
        }
    }
}

impl Downloads {
    fn validate(&self, backend: BotApi) -> Result<(), AppError> {
        require(
            self.roots.len() <= 8 && (!self.enabled || !self.roots.is_empty()),
            "downloads.roots",
        )?;
        let total_ceiling = match backend {
            BotApi::Cloud => 1 << 30,
            BotApi::Local => 64 * (1 << 30),
        };
        require(
            (1..=backend.download_ceiling()).contains(&self.max_file_bytes),
            "downloads.max_file_bytes",
        )?;
        require(
            (self.max_file_bytes..=total_ceiling).contains(&self.max_total_bytes),
            "downloads.max_total_bytes",
        )?;
        require(
            (1..=backend.download_timeout_max()).contains(&self.timeout_seconds),
            "downloads.timeout_seconds",
        )?;
        require(
            (1..=1_024).contains(&self.max_files)
                && (1..=self.max_files.min(64)).contains(&self.concurrent)
                && (1..=604_800).contains(&self.retention_seconds)
                && (1..=3_600).contains(&self.cleanup_interval_seconds)
                && (1..=1_073_741_824).contains(&self.min_free_bytes),
            "downloads.budgets",
        )
    }

    pub(crate) fn normalize(
        &mut self,
        home: Option<&Path>,
        backend: BotApi,
        files: &Files,
        storage: &Storage,
        secrets: [Option<&Path>; 2],
    ) -> Result<(), AppError> {
        self.validate(backend)?;
        for root in &mut self.roots {
            *root = expand_path(root, home)?;
            require(root.parent().is_some(), "downloads.roots")?;
            require(
                !files
                    .allowed_roots
                    .iter()
                    .any(|source| paths_overlap(root, source)),
                "downloads.source separation",
            )?;
            require(
                !storage.path.starts_with(&root)
                    && !secrets
                        .iter()
                        .flatten()
                        .any(|secret| secret.starts_with(&root)),
                "downloads.private storage separation",
            )?;
        }
        for (i, root) in self.roots.iter().enumerate() {
            require(
                !self.roots[..i]
                    .iter()
                    .any(|other| paths_overlap(root, other)),
                "downloads.root separation",
            )?;
        }
        Ok(())
    }
}
