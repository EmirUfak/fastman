use crate::model::{Project, SCHEMA_VERSION};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub name: String,
}

/// Local folder structure: <root>/<uuid>/project.json. Folder/request hierarchy
/// is represented in the JSON tree, not as executable filesystem paths.
#[derive(Debug, Clone)]
pub struct ProjectStore {
    root: PathBuf,
}

impl ProjectStore {
    /// The original project directory remains the default workspace for compatibility.
    pub fn workspaces(&self) -> Result<Vec<Workspace>> {
        let mut workspaces = vec![Workspace {
            id: "default".into(),
            name: "My workspace".into(),
        }];
        let path = self.root.join("workspaces");
        if !path.exists() {
            return Ok(workspaces);
        }
        for entry in fs::read_dir(&path).with_context(|| format!("Read {}", path.display()))? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let id = entry.file_name().to_string_lossy().into_owned();
            if Uuid::parse_str(&id).is_err() {
                continue;
            }
            let metadata = entry.path().join("workspace.json");
            if !metadata.exists() {
                continue;
            }
            let workspace: Workspace = serde_json::from_slice(&fs::read(&metadata)?)
                .with_context(|| format!("Read {}", metadata.display()))?;
            if workspace.id != id || workspace.name.trim().is_empty() {
                bail!("Invalid workspace metadata in {}", metadata.display());
            }
            workspaces.push(workspace);
        }
        workspaces[1..].sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        Ok(workspaces)
    }

    pub fn create_workspace(&self, name: &str) -> Result<Workspace> {
        if name.trim().is_empty() {
            bail!("Workspace name is empty");
        }
        let workspace = Workspace {
            id: Uuid::new_v4().to_string(),
            name: name.trim().into(),
        };
        let dir = self.root.join("workspaces").join(&workspace.id);
        fs::create_dir_all(&dir).with_context(|| format!("Create {}", dir.display()))?;
        fs::write(
            dir.join("workspace.json"),
            serde_json::to_vec_pretty(&workspace)?,
        )
        .context("Write workspace metadata")?;
        Ok(workspace)
    }

    pub fn for_workspace(&self, id: &str) -> Result<Self> {
        if id == "default" {
            return Ok(self.clone());
        }
        let id = Uuid::parse_str(id).context("Invalid workspace ID")?;
        let dir = self.root.join("workspaces").join(id.to_string());
        if !dir.join("workspace.json").is_file() {
            bail!("Workspace not found");
        }
        Ok(Self::at(dir.join("projects")))
    }
    pub fn system_default() -> Result<Self> {
        let root = std::env::var_os("FASTMAN_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::data_local_dir().map(|p| p.join("fastman")))
            .context("No application data directory; set FASTMAN_HOME")?;
        Ok(Self::at(root))
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn create(&self, name: impl Into<String>) -> Result<Project> {
        let project = Project::new(name);
        self.save(&project)?;
        Ok(project)
    }

    pub fn list(&self) -> Result<Vec<Project>> {
        if !self.root.exists() {
            return Ok(vec![]);
        }
        let mut projects = Vec::new();
        for entry in
            fs::read_dir(&self.root).with_context(|| format!("Read {}", self.root.display()))?
        {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let id = entry.file_name();
            // The folder name must be a UUID; ignore unrelated directories.
            if Uuid::parse_str(&id.to_string_lossy()).is_err() {
                continue;
            }
            let path = entry.path().join("project.json");
            if !path.exists() {
                continue;
            }
            projects.push(self.load(&id.to_string_lossy())?);
        }
        projects.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        Ok(projects)
    }

    pub fn load(&self, id: &str) -> Result<Project> {
        let id = Uuid::parse_str(id).context("Invalid project ID")?;
        let path = self.root.join(id.to_string()).join("project.json");
        let bytes = fs::read(&path).with_context(|| format!("Read {}", path.display()))?;
        let project: Project = serde_json::from_slice(&bytes).context("Decode project JSON")?;
        validate_project(&project)?;
        if project.id != id.to_string() {
            bail!("Project ID disagrees with directory");
        }
        Ok(project)
    }

    pub fn save(&self, project: &Project) -> Result<()> {
        validate_project(project)?;
        let id = Uuid::parse_str(&project.id).context("Invalid project ID")?;
        let dir = self.root.join(id.to_string());
        fs::create_dir_all(&dir).with_context(|| format!("Create {}", dir.display()))?;
        let dest = dir.join("project.json");
        let temporary = dir.join(format!(".{}.tmp", Uuid::new_v4()));
        let encoded = serde_json::to_vec_pretty(project)?;
        fs::write(&temporary, encoded).context("Write temporary project file")?;
        // On Windows, rename over an existing file can fail. Avoid deleting the
        // original automatically: a failed replace is safer than data loss.
        if let Err(error) = fs::rename(&temporary, &dest) {
            if dest.exists() {
                let backup = dir.join("project.json.previous");
                let _ = fs::remove_file(&backup);
                fs::rename(&dest, &backup).context("Back up previous project")?;
                if let Err(replace_error) = fs::rename(&temporary, &dest) {
                    let _ = fs::rename(&backup, &dest);
                    return Err(replace_error).context("Replace project file");
                }
                let _ = fs::remove_file(backup);
            } else {
                return Err(error).context("Commit project file");
            }
        }
        Ok(())
    }
}

fn validate_project(project: &Project) -> Result<()> {
    if project.schema_version != SCHEMA_VERSION {
        bail!(
            "Unsupported project schema version {}",
            project.schema_version
        );
    }
    Uuid::parse_str(&project.id).context("Invalid project ID")?;
    if project.name.trim().is_empty() {
        bail!("Project name is empty");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip_and_path_validation() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProjectStore::at(dir.path());
        let project = store.create("API").unwrap();
        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, project.id);
        assert!(store.load("../../etc/passwd").is_err());
        let mut updated = project.clone();
        updated.name = "API renamed".into();
        store.save(&updated).unwrap();
        assert_eq!(store.load(&project.id).unwrap().name, "API renamed");
    }

    #[test]
    fn workspaces_isolate_projects_and_preserve_legacy_store() {
        let dir = tempfile::tempdir().unwrap();
        let root = ProjectStore::at(dir.path());
        let legacy = root.create("Existing").unwrap();
        let workspace = root.create_workspace("Team").unwrap();
        let other = root.for_workspace(&workspace.id).unwrap();
        assert!(other.list().unwrap().is_empty());
        let project = other.create("Other").unwrap();
        assert_eq!(root.list().unwrap()[0].id, legacy.id);
        assert_eq!(other.list().unwrap()[0].id, project.id);
        assert_eq!(root.workspaces().unwrap().len(), 2);
        assert_eq!(
            root.for_workspace("default")
                .unwrap()
                .load(&legacy.id)
                .unwrap()
                .name,
            "Existing"
        );
        assert!(root.for_workspace("../bad").is_err());
        assert!(root.create_workspace(" ").is_err());
    }
}
