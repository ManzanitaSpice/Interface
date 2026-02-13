import { FormEvent, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

type LoaderType = "vanilla" | "forge" | "fabric" | "neoforge" | "quilt";
type InstanceState = "created" | "installing" | "ready" | "running" | "error";
type JavaRuntimePreference = "auto" | "embedded" | "system";
type SettingsTab = "java" | "launcher";

interface InstanceInfo {
  id: string;
  name: string;
  path: string;
  minecraft_version: string;
  loader_type: LoaderType;
  loader_version: string | null;
  state: InstanceState;
}

interface JavaInstallation {
  path: string;
  version: string;
  major: number;
  is_64bit: boolean;
}

interface LauncherSettingsPayload {
  java_runtime: JavaRuntimePreference;
  selected_java_path: string | null;
  embedded_java_available: boolean;
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

const getErrorMessage = (e: unknown) => String(e).replace("Error: ", "");

function LoadingScreen() {
  const [progress, setProgress] = useState(0);

  useEffect(() => {
    const interval = setInterval(() => {
      setProgress((prev) => (prev >= 100 ? 100 : prev + 2));
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

function CreateInstancePage({
  minecraftVersions,
  onInstanceCreated,
}: {
  minecraftVersions: string[];
  onInstanceCreated: (instance: InstanceInfo) => void;
}) {
  const [selectedVersion, setSelectedVersion] = useState("");
  const [selectedLoader, setSelectedLoader] = useState<LoaderType>("vanilla");
  const [loaderVersions, setLoaderVersions] = useState<string[]>([]);
  const [selectedLoaderVersion, setSelectedLoaderVersion] = useState("");
  const [instanceName, setInstanceName] = useState("");
  const [isLoadingVersions, setIsLoadingVersions] = useState(false);
  const [isCreating, setIsCreating] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    if (minecraftVersions.length > 0 && !selectedVersion) setSelectedVersion(minecraftVersions[0]);
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
      } catch (e) {
        if (!isCancelled) {
          setLoaderVersions([]);
          setSelectedLoaderVersion("");
          setError(`No fue posible cargar versiones del loader: ${getErrorMessage(e)}`);
        }
      } finally {
        if (!isCancelled) setIsLoadingVersions(false);
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
      setError(`No se pudo crear la instancia: ${getErrorMessage(e)}`);
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
          <input value={instanceName} onChange={(e) => setInstanceName(e.target.value)} required />
        </div>
        <div className="form-group">
          <label>Minecraft Version</label>
          <select value={selectedVersion} onChange={(e) => setSelectedVersion(e.target.value)}>
            {minecraftVersions.map((v) => (
              <option key={v} value={v}>{v}</option>
            ))}
          </select>
        </div>
        <div className="form-group">
          <label>Mod Loader</label>
          <select value={selectedLoader} onChange={(e) => setSelectedLoader(e.target.value as LoaderType)}>
            {LOADERS.map((l) => (
              <option key={l.value} value={l.value}>{l.label}</option>
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
            {selectedLoader === "vanilla" ? <option value="">No aplica para Vanilla</option> : loaderVersions.map((lv) => (
              <option key={lv} value={lv}>{lv}</option>
            ))}
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

function App() {
  const [isLoading, setIsLoading] = useState(true);
  const [currentView, setCurrentView] = useState<"home" | "create-instance" | "settings">("home");
  const [settingsTab, setSettingsTab] = useState<SettingsTab>("java");
  const [minecraftVersions, setMinecraftVersions] = useState<string[]>([]);
  const [instances, setInstances] = useState<InstanceInfo[]>([]);
  const [javaInstallations, setJavaInstallations] = useState<JavaInstallation[]>([]);
  const [settings, setSettings] = useState<LauncherSettingsPayload | null>(null);
  const [instanceLogs, setInstanceLogs] = useState<Record<string, LogEntry[]>>({});
  const [selectedInstanceId, setSelectedInstanceId] = useState<string | null>(null);
  const [isProfileMenuOpen, setIsProfileMenuOpen] = useState(false);
  const [error, setError] = useState("");

  const selectedInstance = useMemo(
    () => instances.find((instance) => instance.id === selectedInstanceId) ?? null,
    [instances, selectedInstanceId],
  );

  useEffect(() => {
    const timer = setTimeout(() => setIsLoading(false), 1500);
    return () => clearTimeout(timer);
  }, []);

  useEffect(() => {
    const loadData = async () => {
      try {
        const [versions, savedInstances, javas, launcherSettings] = await Promise.all([
          invoke<string[]>("get_minecraft_versions"),
          invoke<InstanceInfo[]>("list_instances"),
          invoke<JavaInstallation[]>("get_java_installations"),
          invoke<LauncherSettingsPayload>("get_launcher_settings"),
        ]);
        setMinecraftVersions(versions);
        setInstances(savedInstances);
        setJavaInstallations(javas);
        setSettings(launcherSettings);
      } catch (e) {
        setError(`No se pudo conectar con el backend: ${getErrorMessage(e)}`);
      }
    };
    void loadData();
  }, []);

  const addLog = (id: string, level: LogEntry["level"], message: string) => {
    setInstanceLogs((prev) => ({
      ...prev,
      [id]: [...(prev[id] ?? []), { timestamp: new Date().toLocaleTimeString(), level, message }],
    }));
  };

  const persistSettings = async (next: LauncherSettingsPayload) => {
    const saved = await invoke<LauncherSettingsPayload>("update_launcher_settings", { payload: next });
    setSettings(saved);
  };

  const handleSaveJavaSettings = async () => {
    if (!settings) return;
    try {
      await persistSettings(settings);
    } catch (e) {
      setError(`No se pudo guardar la configuración de Java: ${getErrorMessage(e)}`);
    }
  };

  const handleStartInstance = async (id: string) => {
    setError("");
    setSelectedInstanceId(id);
    addLog(id, "info", "Solicitud de inicio recibida.");
    const interval = setInterval(() => undefined, 700);

    try {
      await invoke("launch_instance", { id });
      clearInterval(interval);
      setInstances((prev) => prev.map((instance) => (instance.id === id ? { ...instance, state: "running" } : instance)));
      addLog(id, "info", "Instancia iniciada correctamente.");
    } catch (e) {
      clearInterval(interval);
      const details = getErrorMessage(e);
      addLog(id, "error", details);
      setError(`No se pudo iniciar la instancia: ${details}`);
    }
  };

  if (isLoading) return <LoadingScreen />;

  return (
    <div className="app-root">
      <header className="global-topbar">
        <div className="brand-zone"><div className="brand-logo-placeholder" /><span className="brand-title">INTERFACE</span></div>
        <div className="profile-menu-container">
          <button className="profile-btn" onClick={() => setIsProfileMenuOpen((prev) => !prev)} type="button">Perfil ▾</button>
          {isProfileMenuOpen && <div className="profile-dropdown"><button type="button">Mi perfil (próximamente)</button></div>}
        </div>
      </header>

      <div className="app-layout">
        <aside className="sidebar">
          <div className="sidebar-header">Launcher</div>
          <nav>
            <button className={`sidebar-btn ${currentView === "home" ? "active" : ""}`} onClick={() => setCurrentView("home")}>Home</button>
            <button className={`sidebar-btn ${currentView === "settings" ? "active" : ""}`} onClick={() => setCurrentView("settings")}>Configuración</button>
          </nav>
        </aside>

        <main className="content-area">
          {error && <pre className="error-message global-error">{error}</pre>}

          {currentView === "home" && (
            <div>
              <div className="home-toolbar">
                <div><h2>Instances</h2><p>Herramientas rápidas para tus instancias.</p></div>
                <button className="generate-btn toolbar-btn" onClick={() => setCurrentView("create-instance")}>+ Crear instancia</button>
              </div>
              <div className="instance-grid">
                {instances.map((instance) => (
                  <article key={instance.id} className="instance-card">
                    <h3>{instance.name}</h3>
                    <p><strong>Minecraft:</strong> {instance.minecraft_version}</p>
                    <p><strong>Loader:</strong> {instance.loader_type}</p>
                    <p><strong>Status:</strong> {instance.state}</p>
                    <div className="instance-actions">
                      <button className="start-instance-btn" onClick={() => void handleStartInstance(instance.id)}>
                        {instance.state === "running" ? "En ejecución" : "Iniciar instancia"}
                      </button>
                    </div>
                  </article>
                ))}
              </div>
            </div>
          )}

          {currentView === "create-instance" && <CreateInstancePage minecraftVersions={minecraftVersions} onInstanceCreated={(instance) => { setInstances((prev) => [instance, ...prev]); setCurrentView("home"); }} />}

          {currentView === "settings" && (
            <section className="settings-page">
              <h2>Configurador del launcher</h2>
              <div className="settings-tabs">
                <button className={`settings-tab ${settingsTab === "java" ? "active" : ""}`} onClick={() => setSettingsTab("java")}>Java</button>
                <button className={`settings-tab ${settingsTab === "launcher" ? "active" : ""}`} onClick={() => setSettingsTab("launcher")}>Launcher</button>
              </div>

              {settingsTab === "java" && settings && (
                <div className="settings-panel">
                  <h3>Motor de Java para instancias nuevas</h3>
                  <label className="radio-row"><input type="radio" checked={settings.java_runtime === "auto"} onChange={() => setSettings({ ...settings, java_runtime: "auto" })} />Auto (detectar por versión)</label>
                  <label className="radio-row"><input type="radio" checked={settings.java_runtime === "system"} onChange={() => setSettings({ ...settings, java_runtime: "system" })} />Java del sistema</label>
                  <label className="radio-row"><input type="radio" checked={settings.java_runtime === "embedded"} onChange={() => setSettings({ ...settings, java_runtime: "embedded" })} />Java embebido del launcher {settings.embedded_java_available ? "(disponible)" : "(no encontrado)"}</label>

                  <div className="form-group">
                    <label>Ruta Java del sistema</label>
                    <select
                      value={settings.selected_java_path ?? ""}
                      onChange={(e) => setSettings({ ...settings, selected_java_path: e.target.value || null })}
                      disabled={settings.java_runtime !== "system"}
                    >
                      <option value="">Selecciona una instalación detectada</option>
                      {javaInstallations.map((java) => (
                        <option key={java.path} value={java.path}>{`Java ${java.major} (${java.version}) ${java.is_64bit ? "64-bit" : "32-bit"} - ${java.path}`}</option>
                      ))}
                    </select>
                  </div>

                  <button className="generate-btn" type="button" onClick={() => void handleSaveJavaSettings()}>Guardar configuración Java</button>
                </div>
              )}

              {settingsTab === "launcher" && <div className="settings-panel"><p>Próximamente: más opciones globales del launcher.</p></div>}
            </section>
          )}
        </main>
      </div>

      {selectedInstance && (
        <section className="instance-log-panel">
          <div className="instance-log-stream">
            {(instanceLogs[selectedInstance.id] ?? []).map((entry, idx) => (
              <div key={`${selectedInstance.id}-${idx}`} className={`log-entry ${entry.level}`}><span>[{entry.timestamp}]</span><span>{entry.message}</span></div>
            ))}
          </div>
        </section>
      )}
    </div>
  );
}

export default App;
