import { FormEvent, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

type LoaderType = "vanilla" | "forge" | "fabric" | "neoforge" | "quilt";
type InstanceState = "created" | "installing" | "ready" | "running" | "error";

interface InstanceInfo {
  id: string;
  name: string;
  path: string;
  minecraft_version: string;
  loader_type: LoaderType;
  loader_version: string | null;
  state: InstanceState;
}

interface InstanceActionProgress {
  progress: number;
  status: string;
  details: string;
}

interface CreateInstancePayload {
  name: string;
  minecraft_version: string;
  loader_type: LoaderType;
  loader_version: string | null;
  memory_max_mb?: number;
}

interface LogEntry {
  timestamp: string;
  level: "info" | "error" | "warn";
  message: string;
}

const LOADERS: { label: string; value: LoaderType }[] = [
  { label: "Vanilla", value: "vanilla" },
  { label: "Forge", value: "forge" },
  { label: "Fabric", value: "fabric" },
  { label: "NeoForge", value: "neoforge" },
  { label: "Quilt", value: "quilt" },
];

const getErrorMessage = (e: unknown) => {
  const text = String(e);
  return text.replace("Error: ", "");
};

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
    if (!selectedVersion || selectedLoader === "vanilla") {
      setLoaderVersions([]);
      setSelectedLoaderVersion("");
      return;
    }

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

        if (versions.length === 0) {
          setError("No se encontraron versiones estables para esta combinación de loader y Minecraft.");
        }
      } catch (e) {
        if (isCancelled) return;
        setLoaderVersions([]);
        setSelectedLoaderVersion("");
        setError(`No fue posible cargar versiones reales del loader. Detalle: ${getErrorMessage(e)}`);
      } finally {
        if (!isCancelled) {
          setIsLoadingVersions(false);
        }
      }
    };

    void load();

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
      setError(
        `No se pudo crear la instancia. Revisa la configuración y vuelve a intentarlo.\nDetalle técnico: ${getErrorMessage(e)}`,
      );
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

        {error && <pre className="error-message">{error}</pre>}

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
  const [instanceProgress, setInstanceProgress] = useState<Record<string, InstanceActionProgress>>({});
  const [instanceLogs, setInstanceLogs] = useState<Record<string, LogEntry[]>>({});
  const [selectedInstanceId, setSelectedInstanceId] = useState<string | null>(null);
  const [isProfileMenuOpen, setIsProfileMenuOpen] = useState(false);
  const [error, setError] = useState("");

  const selectedInstance = useMemo(
    () => instances.find((instance) => instance.id === selectedInstanceId) ?? null,
    [instances, selectedInstanceId],
  );

  useEffect(() => {
    const timer = setTimeout(() => {
      setIsLoading(false);
    }, 1500);
    return () => clearTimeout(timer);
  }, []);

  useEffect(() => {
    const onEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;

      if (selectedInstanceId) {
        setSelectedInstanceId(null);
      } else if (isProfileMenuOpen) {
        setIsProfileMenuOpen(false);
      } else if (currentView !== "home") {
        setCurrentView("home");
      }
    };

    window.addEventListener("keydown", onEscape);
    return () => window.removeEventListener("keydown", onEscape);
  }, [selectedInstanceId, isProfileMenuOpen, currentView]);

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
        setError(`No se pudo conectar con el backend.\nDetalle técnico: ${getErrorMessage(e)}`);
      }
    };

    void loadData();
  }, []);

  const addLog = (id: string, level: LogEntry["level"], message: string) => {
    setInstanceLogs((prev) => ({
      ...prev,
      [id]: [
        ...(prev[id] ?? []),
        {
          timestamp: new Date().toLocaleTimeString(),
          level,
          message,
        },
      ],
    }));
  };

  const handleInstanceCreated = (instance: InstanceInfo) => {
    setInstances((prev) => [instance, ...prev]);
    setCurrentView("home");
  };

  const handleDeleteInstance = async (id: string) => {
    setError("");
    try {
      await invoke("delete_instance", { id });
      setInstances((prev) => prev.filter((instance) => instance.id !== id));
      setInstanceProgress((prev) => {
        const copy = { ...prev };
        delete copy[id];
        return copy;
      });
      setInstanceLogs((prev) => {
        const copy = { ...prev };
        delete copy[id];
        return copy;
      });
      if (selectedInstanceId === id) {
        setSelectedInstanceId(null);
      }
    } catch (e) {
      setError(`No se pudo eliminar la instancia.\nDetalle técnico: ${getErrorMessage(e)}`);
    }
  };

  const handleForceClose = async (id: string) => {
    try {
      await invoke("force_close_instance", { id });
      addLog(id, "warn", "Se ejecutó un cierre forzado de la instancia.");
      setInstances((prev) =>
        prev.map((instance) =>
          instance.id === id ? { ...instance, state: "ready" } : instance,
        ),
      );
    } catch (e) {
      addLog(id, "error", `Falló el cierre forzado: ${getErrorMessage(e)}`);
      setError(`No se pudo forzar el cierre de la instancia.\nDetalle técnico: ${getErrorMessage(e)}`);
    }
  };

  const handleStartInstance = async (id: string) => {
    setError("");
    setSelectedInstanceId(id);
    addLog(id, "info", "Solicitud de inicio recibida.");
    setInstanceProgress((prev) => ({
      ...prev,
      [id]: {
        progress: 5,
        status: "Preparando inicio",
        details: "Validando archivos de la instancia...",
      },
    }));

    let progress = 5;
    const phases = [
      "Descargando/verificando librerías",
      "Preparando assets",
      "Inicializando loader",
      "Lanzando Minecraft",
    ];
    let phaseIndex = 0;

    const interval = setInterval(() => {
      progress = Math.min(progress + 7, 90);
      if (progress > 20 && phaseIndex < phases.length - 1) {
        phaseIndex += 1;
      }

      setInstanceProgress((prev) => ({
        ...prev,
        [id]: {
          progress,
          status: phases[phaseIndex],
          details: `Completado ${progress}%`,
        },
      }));
      addLog(id, "info", `Fase actual: ${phases[phaseIndex]} (${progress}%).`);
    }, 700);

    try {
      await invoke("launch_instance", { id });
      clearInterval(interval);

      setInstanceProgress((prev) => ({
        ...prev,
        [id]: {
          progress: 100,
          status: "Instancia iniciada",
          details: "Proceso de arranque completado.",
        },
      }));

      setInstances((prev) =>
        prev.map((instance) =>
          instance.id === id ? { ...instance, state: "running" } : instance,
        ),
      );
      addLog(id, "info", "Instancia iniciada correctamente.");
    } catch (e) {
      clearInterval(interval);
      const details = getErrorMessage(e);
      setInstanceProgress((prev) => ({
        ...prev,
        [id]: {
          progress: 100,
          status: "Error al iniciar",
          details,
        },
      }));
      addLog(id, "error", `Error al iniciar la instancia: ${details}`);
      setError(`No se pudo iniciar la instancia.\nDetalle técnico: ${details}`);
    }
  };

  const handleOpenInstanceFolder = async (id: string) => {
    try {
      await invoke("open_instance_folder", { id });
    } catch (e) {
      setError(`No se pudo abrir la carpeta de la instancia.\nDetalle técnico: ${getErrorMessage(e)}`);
    }
  };

  if (isLoading) {
    return <LoadingScreen />;
  }

  return (
    <div className="app-root">
      <header className="global-topbar">
        <div className="brand-zone">
          <div className="brand-logo-placeholder" aria-hidden="true" />
          <span className="brand-title">INTERFACE</span>
        </div>

        <div className="profile-menu-container">
          <button
            className="profile-btn"
            onClick={() => setIsProfileMenuOpen((prev) => !prev)}
            type="button"
          >
            Perfil ▾
          </button>
          {isProfileMenuOpen && (
            <div className="profile-dropdown">
              <button type="button">Mi perfil (próximamente)</button>
              <button type="button">Configuración (próximamente)</button>
              <button type="button">Cerrar sesión (próximamente)</button>
            </div>
          )}
        </div>
      </header>

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
          </nav>
        </aside>

        <main className="content-area">
          {error && <pre className="error-message global-error">{error}</pre>}

          {currentView === "home" && (
            <div>
              <div className="home-toolbar">
                <div>
                  <h2>Instances</h2>
                  <p>Herramientas rápidas para tus instancias.</p>
                </div>
                <button className="generate-btn toolbar-btn" onClick={() => setCurrentView("create-instance")}>
                  + Crear instancia
                </button>
              </div>

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
                    <p className="instance-path" title={instance.path}>
                      <strong>Ruta:</strong> {instance.path}
                    </p>

                    <div className="instance-actions">
                      <button
                        className="start-instance-btn"
                        onClick={() => void handleStartInstance(instance.id)}
                        disabled={
                          (instanceProgress[instance.id] && instanceProgress[instance.id].progress < 100) ||
                          instance.state === "running"
                        }
                      >
                        {instance.state === "running" ? "En ejecución" : "Iniciar instancia"}
                      </button>

                      <button
                        className="open-folder-btn"
                        onClick={() => void handleOpenInstanceFolder(instance.id)}
                        type="button"
                      >
                        Abrir carpeta
                      </button>
                    </div>

                    {instanceProgress[instance.id] && (
                      <div className="instance-progress-wrap">
                        <div className="instance-progress-meta">
                          <span>{instanceProgress[instance.id].status}</span>
                          <span>{instanceProgress[instance.id].progress}%</span>
                        </div>
                        <div className="instance-progress-track">
                          <div
                            className="instance-progress-fill"
                            style={{ width: `${instanceProgress[instance.id].progress}%` }}
                          />
                        </div>
                        <small>{instanceProgress[instance.id].details}</small>
                      </div>
                    )}
                  </article>
                ))}

                {instances.length === 0 && (
                  <div className="empty-state">
                    Aún no hay instancias. Usa <strong>Crear instancia</strong> en la barra superior de Home.
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

      {selectedInstance && (
        <section className="instance-log-panel" role="dialog" aria-label="Panel de ejecución de instancia">
          <div className="instance-log-toolbar">
            <div>
              <h3>{selectedInstance.name}</h3>
              <p>
                Estado: <strong>{selectedInstance.state}</strong>
              </p>
            </div>
            <div className="instance-log-actions">
              <button type="button" className="danger-btn" onClick={() => void handleForceClose(selectedInstance.id)}>
                Forzar cierre
              </button>
              <button type="button" className="danger-btn secondary" onClick={() => void handleDeleteInstance(selectedInstance.id)}>
                Eliminar instancia
              </button>
              <button type="button" className="open-folder-btn" onClick={() => setSelectedInstanceId(null)}>
                Cerrar (Esc)
              </button>
            </div>
          </div>

          <div className="instance-log-stream">
            {(instanceLogs[selectedInstance.id] ?? []).map((entry, idx) => (
              <div key={`${selectedInstance.id}-log-${idx}`} className={`log-entry ${entry.level}`}>
                <span>[{entry.timestamp}]</span>
                <span>{entry.message}</span>
              </div>
            ))}
            {(instanceLogs[selectedInstance.id] ?? []).length === 0 && (
              <p className="empty-logs">Aún no hay eventos para esta instancia.</p>
            )}
          </div>
        </section>
      )}
    </div>
  );
}

export default App;
