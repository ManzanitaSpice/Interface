import { ChangeEvent, FormEvent, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

type LoaderType = "vanilla" | "forge" | "fabric" | "neoforge" | "quilt";
type InstanceState = "created" | "installing" | "ready" | "running" | "error";
type JavaRuntimePreference = "auto" | "embedded" | "system";
type SettingsTab = "java" | "launcher";
type InstanceConfigTab = "java" | "arguments";

interface InstanceInfo {
  id: string;
  name: string;
  path: string;
  minecraft_version: string;
  loader_type: LoaderType;
  loader_version: string | null;
  state: InstanceState;
  required_java_major?: number | null;
  java_path?: string | null;
  max_memory_mb?: number;
  jvm_args?: string[];
  game_args?: string[];
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
  data_dir: string;
}

interface FirstLaunchStatus {
  first_launch: boolean;
  suggested_data_dir: string;
}

interface InitializeInstallationPayload {
  target_dir: string;
  create_desktop_shortcut: boolean;
}


interface CreateInstancePayload {
  name: string;
  minecraft_version: string;
  loader_type: LoaderType;
  loader_version: string | null;
  memory_max_mb?: number;
}


interface UpdateInstanceLaunchConfigPayload {
  id: string;
  java_path: string | null;
  max_memory_mb: number;
  jvm_args: string[];
  game_args: string[];
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

  const recommendedLoaderVersion = loaderVersions[0] ?? "";

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
              <option key={lv} value={lv}>{`${lv === recommendedLoaderVersion ? "★ " : ""}${lv}`}</option>
            ))}
          </select>
          {selectedLoader !== "vanilla" && !isLoadingVersions && loaderVersions.length > 0 && (
            <small>Seleccionada automáticamente la versión más reciente compatible con {selectedVersion}.</small>
          )}
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
  const [currentView, setCurrentView] = useState<"home" | "create-instance" | "settings" | "instance-detail">("home");
  const [settingsTab, setSettingsTab] = useState<SettingsTab>("java");
  const [minecraftVersions, setMinecraftVersions] = useState<string[]>([]);
  const [instances, setInstances] = useState<InstanceInfo[]>([]);
  const [javaInstallations, setJavaInstallations] = useState<JavaInstallation[]>([]);
  const [settings, setSettings] = useState<LauncherSettingsPayload | null>(null);
  const [instanceLogs, setInstanceLogs] = useState<Record<string, LogEntry[]>>({});
  const [selectedInstanceId, setSelectedInstanceId] = useState<string | null>(null);
  const [isProfileMenuOpen, setIsProfileMenuOpen] = useState(false);
  const [error, setError] = useState("");
  const [isMigratingDataDir, setIsMigratingDataDir] = useState(false);
  const [firstLaunchStatus, setFirstLaunchStatus] = useState<FirstLaunchStatus | null>(null);
  const [installTargetDir, setInstallTargetDir] = useState("");
  const [acceptTerms, setAcceptTerms] = useState(false);
  const [acceptAdminPrompt, setAcceptAdminPrompt] = useState(false);
  const [createDesktopShortcut, setCreateDesktopShortcut] = useState(true);
  const [isInitializingInstall, setIsInitializingInstall] = useState(false);
  const [installationCompleted, setInstallationCompleted] = useState(false);
  const [isReinstallingLauncher, setIsReinstallingLauncher] = useState(false);
  const launcherDirInputRef = useRef<HTMLInputElement | null>(null);
  const profileMenuRef = useRef<HTMLDivElement | null>(null);
  const [isDetectingJava, setIsDetectingJava] = useState(false);
  const [instanceConfigTab, setInstanceConfigTab] = useState<InstanceConfigTab>("java");
  const [instanceJavaPathInput, setInstanceJavaPathInput] = useState("");
  const [instanceMaxMemoryInput, setInstanceMaxMemoryInput] = useState("4096");
  const [instanceJvmArgsInput, setInstanceJvmArgsInput] = useState("");
  const [instanceGameArgsInput, setInstanceGameArgsInput] = useState("");
  const [isSavingInstanceConfig, setIsSavingInstanceConfig] = useState(false);

  const selectedInstance = useMemo(
    () => instances.find((instance) => instance.id === selectedInstanceId) ?? null,
    [instances, selectedInstanceId],
  );

  useEffect(() => {
    if (!selectedInstance) return;
    setInstanceJavaPathInput(selectedInstance.java_path ?? "");
    setInstanceMaxMemoryInput(String(selectedInstance.max_memory_mb ?? 4096));
    setInstanceJvmArgsInput((selectedInstance.jvm_args ?? []).join("\n"));
    setInstanceGameArgsInput((selectedInstance.game_args ?? []).join("\n"));
  }, [selectedInstance]);

  useEffect(() => {
    const timer = setTimeout(() => setIsLoading(false), 1500);
    return () => clearTimeout(timer);
  }, []);

  useEffect(() => {
    const handlePointerDown = (event: MouseEvent) => {
      if (!profileMenuRef.current?.contains(event.target as Node)) {
        setIsProfileMenuOpen(false);
      }
    };

    document.addEventListener("mousedown", handlePointerDown);
    return () => document.removeEventListener("mousedown", handlePointerDown);
  }, []);

  useEffect(() => {
    const loadData = async () => {
      try {
        const [versions, savedInstances, javas, launcherSettings, firstStatus] = await Promise.all([
          invoke<string[]>("get_minecraft_versions"),
          invoke<InstanceInfo[]>("list_instances"),
          invoke<JavaInstallation[]>("get_java_installations"),
          invoke<LauncherSettingsPayload>("get_launcher_settings"),
          invoke<FirstLaunchStatus>("get_first_launch_status"),
        ]);
        setMinecraftVersions(versions);
        setInstances(savedInstances);
        setJavaInstallations(javas);
        setSettings(launcherSettings);
        setFirstLaunchStatus(firstStatus);
        setInstallTargetDir(firstStatus.suggested_data_dir);
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

  const handleDetectJavaInstallations = async () => {
    setIsDetectingJava(true);
    setError("");
    try {
      const javas = await invoke<JavaInstallation[]>("get_java_installations");
      setJavaInstallations(javas);
    } catch (e) {
      setError(`No se pudo detectar Java: ${getErrorMessage(e)}`);
    } finally {
      setIsDetectingJava(false);
    }
  };


  const runDataDirMigration = async (selected: string) => {
    setIsMigratingDataDir(true);
    try {
      const updated = await invoke<LauncherSettingsPayload>("migrate_launcher_data_dir", {
        payload: { targetDir: selected },
      });
      setSettings(updated);
    } catch (e) {
      setError(`No se pudo migrar la instalación del launcher: ${getErrorMessage(e)}`);
    } finally {
      setIsMigratingDataDir(false);
    }
  };

  const handleSelectLauncherDir = () => {
    setError("");
    launcherDirInputRef.current?.click();
  };

  const handleLauncherDirPicked = async (event: ChangeEvent<HTMLInputElement>) => {
    const first = event.target.files?.[0] as (File & { path?: string }) | undefined;
    const absolutePath = first?.path;
    if (!absolutePath) {
      setError("No se pudo detectar la carpeta seleccionada. Asegúrate de elegir una carpeta con al menos un archivo.");
      return;
    }

    const relativePath = first.webkitRelativePath;
    const selectedFolderName = relativePath.split("/")[0];
    const normalizedAbsolute = absolutePath.split("\\").join("/");
    const marker = `/${selectedFolderName}/`;
    const markerIndex = normalizedAbsolute.indexOf(marker);
    if (markerIndex === -1) {
      setError("No se pudo resolver la ruta de la carpeta seleccionada.");
      return;
    }

    const selectedRoot = normalizedAbsolute.slice(0, markerIndex + marker.length - 1);
    await runDataDirMigration(selectedRoot);
    event.target.value = "";
  };
  const handleStartInstance = async (id: string) => {
    setError("");
    setSelectedInstanceId(id);
    addLog(id, "info", "Solicitud de inicio recibida.");
    try {
      addLog(id, "info", "Validando instancia y preparando archivos...");
      await invoke("launch_instance", { id });
      setInstances((prev) => prev.map((instance) => (instance.id === id ? { ...instance, state: "running" } : instance)));
      addLog(id, "info", "Proceso iniciado. Revisa los logs de descarga y ejecución.");
    } catch (e) {
      const details = getErrorMessage(e);
      addLog(id, "error", details);
      setError(`No se pudo iniciar la instancia: ${details}`);
    }
  };

  const handleForceCloseInstance = async (id: string) => {
    setError("");
    try {
      await invoke("force_close_instance", { id });
      setInstances((prev) => prev.map((instance) => (instance.id === id ? { ...instance, state: "ready" } : instance)));
      addLog(id, "warn", "Instancia detenida de forma forzada por el usuario.");
    } catch (e) {
      setError(`No se pudo cerrar la instancia: ${getErrorMessage(e)}`);
    }
  };

  const handleOpenInstanceFolder = async (id: string) => {
    setError("");
    try {
      await invoke("open_instance_folder", { id });
    } catch (e) {
      setError(`No se pudo abrir la carpeta de la instancia: ${getErrorMessage(e)}`);
    }
  };

  const handleDeleteInstance = async (id: string) => {
    if (!confirm("¿Seguro que deseas eliminar esta instancia? Esta acción no se puede deshacer.")) {
      return;
    }

    setError("");
    try {
      await invoke("delete_instance", { id });
      setInstances((prev) => prev.filter((instance) => instance.id !== id));
      setInstanceLogs((prev) => {
        const next = { ...prev };
        delete next[id];
        return next;
      });
      if (selectedInstanceId === id) {
        setSelectedInstanceId(null);
        setCurrentView("home");
      }
    } catch (e) {
      setError(`No se pudo eliminar la instancia: ${getErrorMessage(e)}`);
    }
  };

  const openInstanceDetail = (id: string) => {
    setSelectedInstanceId(id);
    setCurrentView("instance-detail");
  };

  const handleInitializeInstallation = async () => {
    if (!acceptTerms || !acceptAdminPrompt || !installTargetDir.trim()) return;

    setIsInitializingInstall(true);
    setError("");
    try {
      const payload: InitializeInstallationPayload = {
        target_dir: installTargetDir.trim(),
        create_desktop_shortcut: createDesktopShortcut,
      };
      const updated = await invoke<LauncherSettingsPayload>("initialize_launcher_installation", { payload });
      setSettings(updated);
      setInstallationCompleted(true);
      setFirstLaunchStatus({
        first_launch: true,
        suggested_data_dir: updated.data_dir,
      });
    } catch (e) {
      setError(`No se pudo completar la instalación inicial: ${getErrorMessage(e)}`);
    } finally {
      setIsInitializingInstall(false);
    }
  };

  const handleCompleteWizard = () => {
    setInstallationCompleted(false);
    setFirstLaunchStatus((prev) => (prev ? { ...prev, first_launch: false } : prev));
  };


  const handleSaveInstanceConfig = async () => {
    if (!selectedInstance) return;
    const parsedMemory = Number(instanceMaxMemoryInput);
    if (!Number.isFinite(parsedMemory) || parsedMemory < 512) {
      setError("La memoria mínima permitida es 512 MB.");
      return;
    }

    const payload: UpdateInstanceLaunchConfigPayload = {
      id: selectedInstance.id,
      java_path: instanceJavaPathInput.trim() || null,
      max_memory_mb: Math.trunc(parsedMemory),
      jvm_args: instanceJvmArgsInput.split("\n").map((v) => v.trim()).filter(Boolean),
      game_args: instanceGameArgsInput.split("\n").map((v) => v.trim()).filter(Boolean),
    };

    setIsSavingInstanceConfig(true);
    setError("");
    try {
      const updated = await invoke<InstanceInfo>("update_instance_launch_config", { payload });
      setInstances((prev) => prev.map((instance) => (instance.id === updated.id ? updated : instance)));
      addLog(updated.id, "info", "Configuración de instancia guardada.");
    } catch (e) {
      setError(`No se pudo guardar la configuración de la instancia: ${getErrorMessage(e)}`);
    } finally {
      setIsSavingInstanceConfig(false);
    }
  };

  const handleReinstallLauncher = async () => {
    if (!confirm("Esto borrará completamente la instalación del launcher, cache e instancias locales. ¿Deseas continuar?")) {
      return;
    }

    setIsReinstallingLauncher(true);
    setError("");
    try {
      const updated = await invoke<LauncherSettingsPayload>("reinstall_launcher_completely");
      setSettings(updated);
      setInstances([]);
      setSelectedInstanceId(null);
      setInstanceLogs({});
    } catch (e) {
      setError(`No se pudo reinstalar completamente el launcher: ${getErrorMessage(e)}`);
    } finally {
      setIsReinstallingLauncher(false);
    }
  };


  if (isLoading) return <LoadingScreen />;

  return (
    <div className="app-root">
      <header className="global-topbar">
        <div className="brand-zone"><div className="brand-logo-placeholder" /><span className="brand-title">INTERFACE</span></div>
        <div className="profile-menu-container" ref={profileMenuRef}>
          <button className="profile-btn" onClick={() => setIsProfileMenuOpen((prev) => !prev)} type="button">Perfil ▾</button>
          {isProfileMenuOpen && <div className="profile-dropdown"><button type="button">Mi perfil (próximamente)</button></div>}
        </div>
      </header>

      <div className="app-layout">
        {currentView !== "instance-detail" && (<aside className="sidebar">
          <div className="sidebar-header">Launcher</div>
          <nav>
            <button className={`sidebar-btn ${currentView === "home" ? "active" : ""}`} onClick={() => setCurrentView("home")}>Home</button>
            <button className={`sidebar-btn ${currentView === "settings" ? "active" : ""}`} onClick={() => setCurrentView("settings")}>Configuración</button>
          </nav>
        </aside>)}

        <main className={`content-area ${currentView === "instance-detail" ? "instance-detail-open" : ""}`}>
          {error && <pre className="error-message global-error">{error}</pre>}

          {firstLaunchStatus?.first_launch && (
            <section className="onboarding-overlay">
              <div className="onboarding-card">
                <h2>Configuración inicial del launcher</h2>
                {!installationCompleted ? (
                  <>
                    <p>Antes de usar el launcher debes completar la instalación inicial con permisos administrativos, términos y ruta de instalación.</p>
                    <div className="form-group">
                      <label>Ruta de instalación completa</label>
                      <input value={installTargetDir} onChange={(e) => setInstallTargetDir(e.target.value)} placeholder="Ej: C:/Games/Interface" />
                    </div>
                    <label className="radio-row">
                      <input type="checkbox" checked={acceptAdminPrompt} onChange={(e) => setAcceptAdminPrompt(e.target.checked)} />
                      Confirmo ejecutar con permisos administrativos cuando el sistema lo solicite.
                    </label>
                    <label className="radio-row">
                      <input type="checkbox" checked={acceptTerms} onChange={(e) => setAcceptTerms(e.target.checked)} />
                      Acepto los términos y condiciones del launcher, permisos de archivos, red y actualización.
                    </label>
                    <label className="radio-row">
                      <input type="checkbox" checked={createDesktopShortcut} onChange={(e) => setCreateDesktopShortcut(e.target.checked)} />
                      Crear acceso directo en escritorio.
                    </label>
                    <button className="generate-btn" type="button" disabled={!acceptTerms || !acceptAdminPrompt || !installTargetDir.trim() || isInitializingInstall} onClick={() => void handleInitializeInstallation()}>
                      {isInitializingInstall ? "Instalando launcher..." : "Instalar launcher"}
                    </button>
                  </>
                ) : (
                  <>
                    <p>Instalación completada correctamente. ¿Qué deseas hacer ahora?</p>
                    <div className="instance-log-actions">
                      <button className="generate-btn" type="button" onClick={handleCompleteWizard}>Inicializar launcher</button>
                      <button className="danger-btn secondary" type="button" onClick={() => window.close()}>Terminar</button>
                    </div>
                  </>
                )}
              </div>
            </section>
          )}

          {!firstLaunchStatus?.first_launch && currentView === "home" && (
            <div>
              <div className="home-toolbar">
                <div><h2>Instances</h2><p>Herramientas rápidas para tus instancias.</p></div>
                <button className="generate-btn toolbar-btn" onClick={() => setCurrentView("create-instance")}>+ Crear instancia</button>
              </div>
              <div className="instance-grid">
                {instances.map((instance) => (
                  <article key={instance.id} className="instance-card clickable" onClick={() => openInstanceDetail(instance.id)} role="button" tabIndex={0} onKeyDown={(e) => { if (e.key === "Enter") openInstanceDetail(instance.id); }}>
                    <h3>{instance.name}</h3>
                    <p><strong>Minecraft:</strong> {instance.minecraft_version}</p>
                    <p><strong>Loader:</strong> {instance.loader_type}</p>
                    <p><strong>Java recomendada:</strong> {instance.required_java_major ? `Java ${instance.required_java_major}+` : "Auto"}</p>
                    <p><strong>Status:</strong> {instance.state}</p>

                  </article>
                ))}
              </div>
            </div>
          )}

          {currentView === "create-instance" && <CreateInstancePage minecraftVersions={minecraftVersions} onInstanceCreated={(instance) => { setInstances((prev) => [instance, ...prev]); setCurrentView("home"); }} />}

          {currentView === "instance-detail" && selectedInstance && (
            <section className="settings-page instance-detail-page">
              <div className="home-toolbar">
                <div>
                  <h2>{selectedInstance.name}</h2>
                  <p>Minecraft {selectedInstance.minecraft_version} · {selectedInstance.loader_type}</p>
                </div>
                <button className="generate-btn toolbar-btn" onClick={() => setCurrentView("home")}>← Volver</button>
              </div>

              <div className="instance-detail-shell">
                <section className="instance-log-stream">
                  {(instanceLogs[selectedInstance.id] ?? []).length === 0 ? (
                    <p className="empty-logs">Sin logs todavía. Inicia la instancia para ver salida.</p>
                  ) : (
                    (instanceLogs[selectedInstance.id] ?? []).map((entry, idx) => (
                      <div key={`${selectedInstance.id}-${idx}`} className={`log-entry ${entry.level}`}><span>[{entry.timestamp}]</span><span>{entry.message}</span></div>
                    ))
                  )}
                </section>

                <aside className="instance-tools-sidebar">
                  <h3>Herramientas</h3>
                  <button className="start-instance-btn" type="button" onClick={() => void handleStartInstance(selectedInstance.id)}>
                    {selectedInstance.state === "running" ? "En ejecución" : "Iniciar instancia"}
                  </button>
                  <button className="danger-btn secondary" type="button" onClick={() => void handleForceCloseInstance(selectedInstance.id)} disabled={selectedInstance.state !== "running"}>
                    Parar ejecución
                  </button>
                  <button className="open-folder-btn" type="button" onClick={() => void handleOpenInstanceFolder(selectedInstance.id)}>
                    Abrir carpeta
                  </button>
                  <button className="danger-btn" type="button" onClick={() => void handleDeleteInstance(selectedInstance.id)}>
                    Eliminar instancia
                  </button>

                  <div className="settings-tabs instance-config-tabs">
                    <button className={`settings-tab ${instanceConfigTab === "java" ? "active" : ""}`} onClick={() => setInstanceConfigTab("java")}>Configuración</button>
                    <button className={`settings-tab ${instanceConfigTab === "arguments" ? "active" : ""}`} onClick={() => setInstanceConfigTab("arguments")}>Argumentos</button>
                  </div>

                  {instanceConfigTab === "java" && (
                    <div className="settings-panel">
                      <div className="form-group">
                        <label>Ruta Java específica</label>
                        <input value={instanceJavaPathInput} onChange={(e) => setInstanceJavaPathInput(e.target.value)} placeholder="Auto por compatibilidad" />
                      </div>
                      <div className="form-group">
                        <label>Memoria máxima (MB)</label>
                        <input type="number" min={512} step={256} value={instanceMaxMemoryInput} onChange={(e) => setInstanceMaxMemoryInput(e.target.value)} />
                      </div>
                    </div>
                  )}

                  {instanceConfigTab === "arguments" && (
                    <div className="settings-panel">
                      <div className="form-group">
                        <label>JVM args (1 por línea)</label>
                        <textarea value={instanceJvmArgsInput} onChange={(e) => setInstanceJvmArgsInput(e.target.value)} rows={5} />
                      </div>
                      <div className="form-group">
                        <label>Game args (1 por línea)</label>
                        <textarea value={instanceGameArgsInput} onChange={(e) => setInstanceGameArgsInput(e.target.value)} rows={5} />
                      </div>
                    </div>
                  )}

                  <button className="generate-btn" type="button" onClick={() => void handleSaveInstanceConfig()} disabled={isSavingInstanceConfig}>
                    {isSavingInstanceConfig ? "Guardando..." : "Guardar configuración"}
                  </button>
                </aside>
              </div>

            </section>
          )}

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
                  <label className="radio-row"><input type="radio" checked={settings.java_runtime === "embedded"} disabled={!settings.embedded_java_available} onChange={() => setSettings({ ...settings, java_runtime: "embedded" })} />Java embebido del launcher {settings.embedded_java_available ? "(disponible)" : "(no encontrado)"}</label>
                  {!settings.embedded_java_available && <small>Coloca un runtime Java en <code>{settings.data_dir}/runtime</code> para habilitarlo.</small>}

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

                  <button className="open-folder-btn" type="button" onClick={() => void handleDetectJavaInstallations()} disabled={isDetectingJava}>
                    {isDetectingJava ? "Detectando instalaciones..." : "Detectar instalaciones Java"}
                  </button>

                  <button className="generate-btn" type="button" onClick={() => void handleSaveJavaSettings()}>Guardar configuración Java</button>
                </div>
              )}

              {settingsTab === "launcher" && settings && (
                <div className="settings-panel">
                  <h3>Ruta de datos del launcher</h3>
                  <p>Ubicación actual: <code>{settings.data_dir}</code></p>
                  <input ref={launcherDirInputRef} type="file" className="hidden-dir-picker" style={{ display: "none" }} onChange={(e) => void handleLauncherDirPicked(e)} {...({ webkitdirectory: "", directory: "" } as Record<string, string>)} />
                  <button className="generate-btn" type="button" onClick={handleSelectLauncherDir} disabled={isMigratingDataDir || isReinstallingLauncher}>
                    {isMigratingDataDir ? "Migrando datos..." : "Cambiar carpeta e iniciar migración"}
                  </button>

                  <hr className="settings-separator" />
                  <h3>Reinstalación completa</h3>
                  <p>Borra por completo datos locales, caché y jars para reinstalar de cero.</p>
                  <button className="danger-btn" type="button" onClick={() => void handleReinstallLauncher()} disabled={isReinstallingLauncher || isMigratingDataDir}>
                    {isReinstallingLauncher ? "Reinstalando launcher..." : "Reinstalar launcher completamente"}
                  </button>
                </div>
              )}
            </section>
          )}
        </main>
      </div>

    </div>
  );
}

export default App;
