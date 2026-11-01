use super::*;

impl Downloads {
    pub(crate) fn validate(&self) -> Result<(), AppError> {
        require(
            self.roots.len() <= 8 && (!self.enabled || !self.roots.is_empty()),
            "downloads.roots",
        )?;
        require(
            (1..=CLOUD_DOWNLOAD_BYTES).contains(&self.max_file_bytes)
                && (self.max_file_bytes..=1_073_741_824).contains(&self.max_total_bytes)
                && (1..=1_024).contains(&self.max_files)
                && (1..=self.max_files.min(64)).contains(&self.concurrent)
                && (1..=DOWNLOAD_TIMEOUT_MAX).contains(&self.timeout_seconds)
                && (1..=604_800).contains(&self.retention_seconds)
                && (1..=3_600).contains(&self.cleanup_interval_seconds)
                && (1..=1_073_741_824).contains(&self.min_free_bytes),
            "downloads.budgets",
        )
    }

    pub(crate) fn normalize(
        &mut self,
        home: Option<&Path>,
        files: &Files,
        storage: &Storage,
        secrets: [Option<&Path>; 2],
    ) -> Result<(), AppError> {
        self.validate()?;
        for root in &mut self.roots {
            *root = expand_path(root, home)?;
            require(root.parent().is_some(), "downloads.roots")?;
            require(
                !files
                    .allowed_roots
                    .iter()
                    .any(|source| overlap(root, source)),
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
                !self.roots[..i].iter().any(|other| overlap(root, other)),
                "downloads.root separation",
            )?;
        }
        Ok(())
    }
}

fn overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}
