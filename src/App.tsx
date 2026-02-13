import { FormEvent, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

type LoaderType = "vanilla" | "forge" | "fabric" | "neoforge" | "quilt";
type InstanceState = "created" | "installing" | "ready" | "running" | "error";

interface InstanceInfo {
  id: string;
  name: string;
  minecraft_version: string;
  loader_type: LoaderType;
  loader_version: string | null;
  state: InstanceState;
}

interface CreateInstancePayload {
  name: string;
  minecraft_version: string;
  loader_type: LoaderType;
  loader_version: string | null;
  memory_max_mb?: number;
}

const LOADERS: { label: string; value: LoaderType }[] = [
  { label: "Vanilla", value: "vanilla" },
  { label: "Forge", value: "forge" },
  { label: "Fabric", value: "fabric" },
  { label: "NeoForge", value: "neoforge" },
  { label: "Quilt", value: "quilt" },
];

function LoadingScreen() {
  const [progress, setProgress] = useState(0);

  useEffect(() => {
    const interval = setInterval(() => {
      setProgress((prev) => {
        if (prev >= 100) {
          clearInterval(interval);
          return 100;
        }
        return prev + 2;
      });
    }, 50);

    return () => clearInterval(interval);
  }, []);

  return (
    <div className="loading-screen">
      <h2 className="loading-title">Initializing Launcher...</h2>
      <div className="progress-bar-container">
        <div className="progress-bar-fill" style={{ width: `${progress}%` }} />
      </div>
      <p className="loading-progress">{progress}%</p>
    </div>
  );
}

interface CreateInstancePageProps {
  minecraftVersions: string[];
  onInstanceCreated: (instance: InstanceInfo) => void;
}

function CreateInstancePage({ minecraftVersions, onInstanceCreated }: CreateInstancePageProps) {
  const [selectedVersion, setSelectedVersion] = useState("");
  const [selectedLoader, setSelectedLoader] = useState<LoaderType>("vanilla");
  const [loaderVersions, setLoaderVersions] = useState<string[]>([]);
  const [selectedLoaderVersion, setSelectedLoaderVersion] = useState("");
  const [instanceName, setInstanceName] = useState("");
  const [isLoadingVersions, setIsLoadingVersions] = useState(false);
  const [isCreating, setIsCreating] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    if (minecraftVersions.length > 0 && !selectedVersion) {
      setSelectedVersion(minecraftVersions[0]);
    }
  }, [minecraftVersions, selectedVersion]);

  useEffect(() => {
    if (!selectedVersion) return;

    let isCancelled = false;
    const load = async () => {
      setIsLoadingVersions(true);
      setError("");
      try {
        const versions = await invoke<string[]>("get_loader_versions", {
          loaderType: selectedLoader,
          minecraftVersion: selectedVersion,
        });

        if (isCancelled) return;

        setLoaderVersions(versions);
        setSelectedLoaderVersion(versions[0] ?? "");
      } catch (e) {
        if (isCancelled) return;
        setLoaderVersions([]);
        setSelectedLoaderVersion("");
        setError(`No se pudieron cargar versiones del loader: ${String(e)}`);
      } finally {
        if (!isCancelled) {
          setIsLoadingVersions(false);
        }
      }
    };

    load();

    return () => {
      isCancelled = true;
    };
  }, [selectedLoader, selectedVersion]);

  const canSubmit = useMemo(() => {
    if (!instanceName.trim() || !selectedVersion) return false;
    if (selectedLoader === "vanilla") return true;
    return Boolean(selectedLoaderVersion);
  }, [instanceName, selectedVersion, selectedLoader, selectedLoaderVersion]);

  const handleGenerate = async (e: FormEvent) => {
    e.preventDefault();
    if (!canSubmit) return;

    setIsCreating(true);
    setError("");

    try {
      const payload: CreateInstancePayload = {
        name: instanceName.trim(),
        minecraft_version: selectedVersion,
        loader_type: selectedLoader,
        loader_version: selectedLoader === "vanilla" ? null : selectedLoaderVersion,
        memory_max_mb: 4096,
      };

      const created = await invoke<InstanceInfo>("create_instance", { payload });
      onInstanceCreated(created);
      setInstanceName("");
    } catch (e) {
      setError(`No se pudo crear la instancia: ${String(e)}`);
    } finally {
      setIsCreating(false);
    }
  };

  return (
    <div className="create-instance-page">
      <h2>Create New Instance</h2>
      <form className="create-instance-form" onSubmit={handleGenerate}>
        <div className="form-group">
          <label>Instance Name</label>
          <input
            type="text"
            placeholder="My New Modpack"
            value={instanceName}
            onChange={(e) => setInstanceName(e.target.value)}
            required
          />
        </div>

        <div className="form-group">
          <label>Minecraft Version</label>
          <select value={selectedVersion} onChange={(e) => setSelectedVersion(e.target.value)}>
            {minecraftVersions.map((v) => (
              <option key={v} value={v}>
                {v}
              </option>
            ))}
          </select>
        </div>

        <div className="form-group">
          <label>Mod Loader</label>
          <select
            value={selectedLoader}
            onChange={(e) => setSelectedLoader(e.target.value as LoaderType)}
          >
            {LOADERS.map((l) => (
              <option key={l.value} value={l.value}>
                {l.label}
              </option>
            ))}
          </select>
        </div>

        <div className="form-group">
          <label>Loader Version</label>
          <select
            value={selectedLoaderVersion}
            onChange={(e) => setSelectedLoaderVersion(e.target.value)}
            disabled={selectedLoader === "vanilla" || isLoadingVersions || loaderVersions.length === 0}
          >
            {selectedLoader === "vanilla" ? (
              <option value="">No aplica para Vanilla</option>
            ) : (
              loaderVersions.map((lv) => (
                <option key={lv} value={lv}>
                  {lv}
                </option>
              ))
            )}
          </select>
        </div>

        {error && <p className="error-message">{error}</p>}

        <button type="submit" className="generate-btn" disabled={!canSubmit || isCreating}>
          {isCreating ? "Creating..." : "Generate Instance"}
        </button>
      </form>
    </div>
  );
}

function formatLoader(loader: LoaderType) {
  return loader === "neoforge"
    ? "NeoForge"
    : loader.charAt(0).toUpperCase() + loader.slice(1);
}

function App() {
  const [isLoading, setIsLoading] = useState(true);
  const [currentView, setCurrentView] = useState("home");
  const [minecraftVersions, setMinecraftVersions] = useState<string[]>([]);
  const [instances, setInstances] = useState<InstanceInfo[]>([]);
  const [error, setError] = useState("");

  useEffect(() => {
    const timer = setTimeout(() => {
      setIsLoading(false);
    }, 1500);
    return () => clearTimeout(timer);
  }, []);

  useEffect(() => {
    const loadData = async () => {
      try {
        const [versions, savedInstances] = await Promise.all([
          invoke<string[]>("get_minecraft_versions"),
          invoke<InstanceInfo[]>("list_instances"),
        ]);

        setMinecraftVersions(versions);
        setInstances(savedInstances);
      } catch (e) {
        setError(`No se pudo conectar al backend: ${String(e)}`);
      }
    };

    loadData();
  }, []);

  const handleInstanceCreated = (instance: InstanceInfo) => {
    setInstances((prev) => [instance, ...prev]);
    setCurrentView("home");
  };

  if (isLoading) {
    return <LoadingScreen />;
  }

  return (
    <div className="app-layout">
      <aside className="sidebar">
        <div className="sidebar-header">Launcher</div>
        <nav>
          <button
            className={`sidebar-btn ${currentView === "home" ? "active" : ""}`}
            onClick={() => setCurrentView("home")}
          >
            Home
          </button>
          <button
            className={`sidebar-btn ${currentView === "create-instance" ? "active" : ""}`}
            onClick={() => setCurrentView("create-instance")}
          >
            Create Instance
          </button>
        </nav>
      </aside>

      <main className="content-area">
        {error && <p className="error-message">{error}</p>}

        {currentView === "home" && (
          <div>
            <h2>Instances</h2>
            <p>Tus instancias creadas aparecen aquí con información real del backend.</p>
            <div className="instance-grid">
              {instances.map((instance) => (
                <article key={instance.id} className="instance-card">
                  <h3>{instance.name}</h3>
                  <p>
                    <strong>Minecraft:</strong> {instance.minecraft_version}
                  </p>
                  <p>
                    <strong>Loader:</strong> {formatLoader(instance.loader_type)}
                  </p>
                  <p>
                    <strong>Loader Version:</strong> {instance.loader_version ?? "N/A"}
                  </p>
                  <p>
                    <strong>Status:</strong> {instance.state}
                  </p>
                </article>
              ))}

              {instances.length === 0 && (
                <div className="empty-state">
                  Aún no hay instancias. Ve a <strong>Create Instance</strong> para crear una real.
                </div>
              )}
            </div>
          </div>
        )}

        {currentView === "create-instance" && (
          <CreateInstancePage
            minecraftVersions={minecraftVersions}
            onInstanceCreated={handleInstanceCreated}
          />
        )}
      </main>
    </div>
  );
}

export default App;
