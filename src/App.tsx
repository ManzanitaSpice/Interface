import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import "./styles/app/base.css";
import "./styles/app/layout.css";
import "./styles/app/topbar.css";
import "./styles/app/buttons.css";
import "./styles/app/sidebars.css";
import "./styles/app/instances.css";
import "./styles/app/settings.css";

type LoaderType = "vanilla" | "forge" | "fabric" | "neoforge" | "quilt";
type TopSection = "menu" | "instances" | "news" | "explorer" | "servers" | "community" | "global-settings";
type InstanceAction = "Iniciar" | "Forzar Cierre" | "Editar" | "Cambiar Grupo" | "Carpeta" | "Exportar" | "Copiar" | "Borrar" | "Crear Atajo";
type EditSection = "Ejecucion" | "Version" | "Mods" | "ResourcePacks" | "Shader Packs" | "Notas" | "Mundos" | "Servidores" | "Capturas" | "Configuracion" | "Otros Registros";
type AppMode = "main" | "create";
type InstanceConfigTab = "General" | "Java" | "Ajustes" | "Comandos Personalizados" | "Variables de entorno";

interface InstanceInfo {
  id: string;
  name: string;
  minecraft_version: string;
  loader_type: LoaderType;
  loader_version: string | null;
  state: "creating" | "ready" | "running" | "stopped" | "error";
  icon_path?: string | null;
  total_size_bytes: number;
}

interface DeleteInstanceResponse {
  status: "deleted" | "needs_elevation" | "elevation_requested";
}

interface LaunchProgressEvent {
  id: string;
  value: number;
  stage: string;
  state: "idle" | "running" | "done" | "error";
}

interface LaunchLogEvent {
  id: string;
  level: "info" | "warn" | "error";
  message: string;
}

interface CreateProgressEvent {
  id: string;
  value: number;
  stage: string;
  state: "idle" | "running" | "done" | "error";
}

interface CreateLogEvent {
  id: string;
  level: "info" | "warn" | "error";
  message: string;
}

interface MinecraftVersionEntry {
  id: string;
  release_time: string;
  version_type: string;
}

interface LoaderVersionEntry {
  version: string;
  stable: boolean;
  source: "official" | "fallback";
}

type MinecraftVersionFilter = "all" | "playable";

const SECTION_LABELS: { key: TopSection; label: string }[] = [
  { key: "menu", label: "Menu" },
  { key: "instances", label: "Mis Instancias" },
  { key: "news", label: "Noticias" },
  { key: "explorer", label: "Explorador" },
  { key: "servers", label: "Servidores" },
  { key: "community", label: "Comunidad" },
  { key: "global-settings", label: "Configuracion Global" },
];

const INSTANCE_ACTIONS: InstanceAction[] = [
  "Iniciar",
  "Forzar Cierre",
  "Editar",
  "Cambiar Grupo",
  "Carpeta",
  "Exportar",
  "Copiar",
  "Borrar",
  "Crear Atajo",
];

const EDIT_SECTIONS: EditSection[] = [
  "Ejecucion",
  "Version",
  "Mods",
  "ResourcePacks",
  "Shader Packs",
  "Notas",
  "Mundos",
  "Servidores",
  "Capturas",
  "Configuracion",
  "Otros Registros",
];

const CREATE_SECTIONS = [
  "Base",
  "Version",
  "Loader",
  "Java",
  "Memoria",
  "Mods",
  "Recursos",
  "Revision",
] as const;

const LOADER_CHOICES: { value: LoaderType | "liteloader"; label: string; supported: boolean }[] = [
  { value: "vanilla", label: "Vanilla", supported: true },
  { value: "neoforge", label: "NeoForge", supported: true },
  { value: "forge", label: "Forge", supported: true },
  { value: "fabric", label: "Fabric", supported: true },
  { value: "quilt", label: "Quilt", supported: true },
  { value: "liteloader", label: "LiteLoader", supported: false },
];

const INSTANCE_CONFIG_TABS: InstanceConfigTab[] = [
  "General",
  "Java",
  "Ajustes",
  "Comandos Personalizados",
  "Variables de entorno",
];

const formatBytes = (bytes: number) => {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  const power = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  return `${(bytes / 1024 ** power).toFixed(power === 0 ? 0 : 2)} ${units[power]}`;
};

const prettyLoader = (loader: LoaderType) => loader.charAt(0).toUpperCase() + loader.slice(1);

const MOJANG_VERSION_MANIFEST_URL = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const FABRIC_LOADER_URL = "https://meta.fabricmc.net/v2/versions/loader";
const FORGE_MAVEN_METADATA_URL = "https://maven.minecraftforge.net/net/minecraftforge/forge/maven-metadata.xml";
const NEOFORGE_MAVEN_METADATA_URL = "https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml";
const QUILT_LOADER_URL = "https://meta.quiltmc.org/v3/versions/loader";

function App() {
  const [activeSection, setActiveSection] = useState<TopSection>("instances");
  const [instances, setInstances] = useState<InstanceInfo[]>([]);
  const [selectedInstance, setSelectedInstance] = useState<InstanceInfo | null>(null);
  const [showInstancePanel, setShowInstancePanel] = useState(false);
  const [editingInstance, setEditingInstance] = useState<InstanceInfo | null>(null);
  const [activeEditSection, setActiveEditSection] = useState<EditSection>("Ejecucion");
  const [appMode, setAppMode] = useState<AppMode>("main");
  const [activeCreateSection, setActiveCreateSection] = useState<(typeof CREATE_SECTIONS)[number]>("Base");
  const [activeInstanceConfigTab, setActiveInstanceConfigTab] = useState<InstanceConfigTab>("General");
  const [launchProgress, setLaunchProgress] = useState<LaunchProgressEvent | null>(null);
  const [launchLogs, setLaunchLogs] = useState<LaunchLogEvent[]>([]);
  const [launchError, setLaunchError] = useState<string | null>(null);
  const [instanceSearch, setInstanceSearch] = useState("");
  const [showProfileMenu, setShowProfileMenu] = useState(false);
  const [showSortMenu, setShowSortMenu] = useState(false);
  const [showMoreMenu, setShowMoreMenu] = useState(false);
  const [showSearchInput, setShowSearchInput] = useState(false);
  const [expandedInstanceId, setExpandedInstanceId] = useState<string | null>(null);
  const [pendingDeleteInstance, setPendingDeleteInstance] = useState<InstanceInfo | null>(null);
  const [deleteInProgress, setDeleteInProgress] = useState(false);
  const [deleteFeedback, setDeleteFeedback] = useState<{ type: "idle" | "progress" | "success" | "error"; message: string; needsElevation?: boolean }>({
    type: "idle",
    message: "",
  });
  const [minecraftVersions, setMinecraftVersions] = useState<MinecraftVersionEntry[]>([]);
  const [minecraftVersionsLoading, setMinecraftVersionsLoading] = useState(false);
  const [minecraftVersionsError, setMinecraftVersionsError] = useState<string | null>(null);
  const [minecraftFilter, setMinecraftFilter] = useState<MinecraftVersionFilter>("all");
  const [selectedMinecraftVersion, setSelectedMinecraftVersion] = useState<string | null>(null);
  const [selectedLoaderType, setSelectedLoaderType] = useState<LoaderType | null>("vanilla");
  const [loaderVersions, setLoaderVersions] = useState<LoaderVersionEntry[]>([]);
  const [loaderVersionsLoading, setLoaderVersionsLoading] = useState(false);
  const [loaderVersionsError, setLoaderVersionsError] = useState<string | null>(null);
  const [selectedLoaderVersion, setSelectedLoaderVersion] = useState<string | null>(null);
  const [newInstanceName, setNewInstanceName] = useState("");
  const [createInProgress, setCreateInProgress] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);
  const [createProgress, setCreateProgress] = useState<CreateProgressEvent | null>(null);
  const [createLogs, setCreateLogs] = useState<CreateLogEvent[]>([]);
  const executionLogRef = useRef<HTMLDivElement | null>(null);
  const profileMenuRef = useRef<HTMLDivElement | null>(null);
  const sortMenuRef = useRef<HTMLDivElement | null>(null);
  const moreMenuRef = useRef<HTMLDivElement | null>(null);
  const searchInputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    const loadInstances = async () => {
      try {
        const saved = await invoke<InstanceInfo[]>("list_instances");
        setInstances(saved);
      } catch {
        setInstances([]);
      }
    };
    void loadInstances();
  }, []);

  useEffect(() => {
    const loadMinecraftVersions = async () => {
      setMinecraftVersionsLoading(true);
      setMinecraftVersionsError(null);
      try {
        const versions = await invoke<MinecraftVersionEntry[]>("get_minecraft_versions_detailed");
        setMinecraftVersions(
          versions.sort((a, b) => new Date(b.release_time).getTime() - new Date(a.release_time).getTime()),
        );
      } catch {
        try {
          const response = await fetch(MOJANG_VERSION_MANIFEST_URL);
          if (!response.ok) {
            throw new Error(`HTTP ${response.status}`);
          }
          const data = await response.json() as { versions?: MinecraftVersionEntry[] };
          const officialVersions = (data.versions ?? [])
            .filter((entry) => !entry.id.toLowerCase().includes("demo"))
            .sort((a, b) => new Date(b.release_time).getTime() - new Date(a.release_time).getTime());
          setMinecraftVersions(officialVersions);
        } catch {
          setMinecraftVersions([]);
          setMinecraftVersionsError("No se pudieron cargar las versiones oficiales de Minecraft.");
        }
      } finally {
        setMinecraftVersionsLoading(false);
      }
    };

    void loadMinecraftVersions();
  }, []);

  useEffect(() => {
    if (!selectedMinecraftVersion && minecraftVersions.length > 0) {
      setSelectedMinecraftVersion(minecraftVersions[0].id);
    }
  }, [minecraftVersions, selectedMinecraftVersion]);

  useEffect(() => {
    if (!selectedMinecraftVersion || !selectedLoaderType || selectedLoaderType === "vanilla") {
      setLoaderVersions([]);
      setSelectedLoaderVersion(selectedLoaderType === "vanilla" ? "integrado" : null);
      setLoaderVersionsError(null);
      setLoaderVersionsLoading(false);
      return;
    }

    const loadLoaderVersions = async () => {
      setLoaderVersionsLoading(true);
      setLoaderVersionsError(null);
      try {
        const versions = await invoke<string[]>("get_loader_versions", {
          loaderType: selectedLoaderType,
          minecraftVersion: selectedMinecraftVersion,
        });
        const normalized = versions.map((version, index) => ({
          version,
          stable: index === 0,
          source: "official" as const,
        }));
        setLoaderVersions(normalized);
        setSelectedLoaderVersion(normalized[0]?.version ?? null);
      } catch {
        try {
          let fallbackVersions: LoaderVersionEntry[] = [];

          if (selectedLoaderType === "fabric") {
            const response = await fetch(`${FABRIC_LOADER_URL}/${selectedMinecraftVersion}`);
            const payload = await response.json() as Array<{ loader?: { version?: string }; stable?: boolean }>;
            fallbackVersions = payload
              .filter((entry) => entry.loader?.version)
              .map((entry) => ({
                version: entry.loader?.version ?? "",
                stable: Boolean(entry.stable),
                source: "fallback",
              }));
          }

          if (selectedLoaderType === "quilt") {
            const response = await fetch(QUILT_LOADER_URL);
            const payload = await response.json() as Array<{ loader?: { version?: string }; stable?: boolean }>; 
            fallbackVersions = payload
              .filter((entry) => entry.loader?.version)
              .map((entry) => ({
                version: entry.loader?.version ?? "",
                stable: Boolean(entry.stable),
                source: "fallback",
              }));
          }

          if (selectedLoaderType === "forge" || selectedLoaderType === "neoforge") {
            const metadataUrl = selectedLoaderType === "forge" ? FORGE_MAVEN_METADATA_URL : NEOFORGE_MAVEN_METADATA_URL;
            const response = await fetch(metadataUrl);
            const xmlText = await response.text();
            const parser = new DOMParser();
            const xml = parser.parseFromString(xmlText, "application/xml");
            const versions = Array.from(xml.querySelectorAll("version"))
              .map((entry) => entry.textContent?.trim() ?? "")
              .filter(Boolean)
              .filter((version) => selectedLoaderType === "forge"
                ? version.startsWith(`${selectedMinecraftVersion}-`)
                : version.includes(selectedMinecraftVersion.replace("1.", "").split(".").slice(0, 2).join("."))
              )
              .map((version) => selectedLoaderType === "forge" ? version.replace(`${selectedMinecraftVersion}-`, "") : version);

            fallbackVersions = versions.map((version, index) => ({ version, stable: index === 0, source: "fallback" }));
          }

          setLoaderVersions(fallbackVersions);
          setSelectedLoaderVersion(fallbackVersions[0]?.version ?? null);
          if (fallbackVersions.length === 0) {
            setLoaderVersionsError("No se encontraron versiones oficiales para este loader y versión de Minecraft.");
          }
        } catch {
          setLoaderVersions([]);
          setSelectedLoaderVersion(null);
          setLoaderVersionsError("No se pudieron cargar versiones de loaders desde APIs oficiales.");
        }
      } finally {
        setLoaderVersionsLoading(false);
      }
    };

    void loadLoaderVersions();
  }, [selectedLoaderType, selectedMinecraftVersion]);

  useEffect(() => {
    let mounted = true;
    const listeners: UnlistenFn[] = [];

    const setupListeners = async () => {
      const unlistenProgress = await listen<LaunchProgressEvent>("instance-launch-progress", (event) => {
        if (!mounted) return;
        if (selectedInstance && event.payload.id !== selectedInstance.id) return;
        setLaunchProgress(event.payload);
      });

      const unlistenLog = await listen<LaunchLogEvent>("instance-launch-log", (event) => {
        if (!mounted) return;
        if (selectedInstance && event.payload.id !== selectedInstance.id) return;
        setLaunchLogs((prev) => [...prev.slice(-100), event.payload]);
      });

      const unlistenCreateProgress = await listen<CreateProgressEvent>("instance-create-progress", (event) => {
        if (!mounted) return;
        setCreateProgress(event.payload);
      });

      const unlistenCreateLog = await listen<CreateLogEvent>("instance-create-log", (event) => {
        if (!mounted) return;
        setCreateLogs((prev) => [...prev.slice(-100), event.payload]);
      });

      listeners.push(unlistenProgress, unlistenLog, unlistenCreateProgress, unlistenCreateLog);
    };

    void setupListeners();

    return () => {
      mounted = false;
      listeners.forEach((unlisten) => unlisten());
    };
  }, [selectedInstance]);

  useEffect(() => {
    if (!executionLogRef.current) return;
    executionLogRef.current.scrollTop = executionLogRef.current.scrollHeight;
  }, [launchLogs, launchProgress, launchError]);

  useEffect(() => {
    const onDocumentClick = (event: MouseEvent) => {
      if (!profileMenuRef.current) return;
      if (!profileMenuRef.current.contains(event.target as Node)) {
        setShowProfileMenu(false);
      }

      if (sortMenuRef.current && !sortMenuRef.current.contains(event.target as Node)) {
        setShowSortMenu(false);
      }

      if (moreMenuRef.current && !moreMenuRef.current.contains(event.target as Node)) {
        setShowMoreMenu(false);
      }
    };

    const onEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      if (showProfileMenu) {
        setShowProfileMenu(false);
        return;
      }
      if (showInstancePanel) {
        setShowInstancePanel(false);
        return;
      }
      if (editingInstance) {
        setEditingInstance(null);
        return;
      }
      if (showSortMenu) {
        setShowSortMenu(false);
        return;
      }
      if (showMoreMenu) {
        setShowMoreMenu(false);
        return;
      }
      if (appMode === "create") {
        setAppMode("main");
      }
    };

    window.addEventListener("click", onDocumentClick);
    window.addEventListener("keydown", onEscape);
    return () => {
      window.removeEventListener("click", onDocumentClick);
      window.removeEventListener("keydown", onEscape);
    };
  }, [appMode, editingInstance, showInstancePanel, showProfileMenu, showSortMenu, showMoreMenu]);

  useEffect(() => {
    if (!showSearchInput) return;
    searchInputRef.current?.focus();
  }, [showSearchInput]);

  const instanceCards = useMemo(() => {
    const query = instanceSearch.trim().toLowerCase();
    if (!query) return instances;

    return instances.filter((instance) =>
      [instance.name, instance.minecraft_version, prettyLoader(instance.loader_type)]
        .join(" ")
        .toLowerCase()
        .includes(query),
    );
  }, [instances, instanceSearch]);

  const enterEditMode = () => {
    if (!selectedInstance) return;
    setEditingInstance(selectedInstance);
    setShowInstancePanel(false);
  };

  const reloadInstances = async () => {
    const saved = await invoke<InstanceInfo[]>("list_instances");
    setInstances(saved);
    if (selectedInstance) {
      const updated = saved.find((instance) => instance.id === selectedInstance.id) ?? null;
      setSelectedInstance(updated);
      if (!updated) {
        setShowInstancePanel(false);
        setEditingInstance(null);
      }
    }
  };

  const copyInstance = async () => {
    if (!selectedInstance) return;
    await invoke("clone_instance", { id: selectedInstance.id });
    await reloadInstances();
  };

  const deleteInstanceById = async (instanceToDelete: InstanceInfo, requestElevation = false) => {
    if (!instanceToDelete) return;

    setDeleteInProgress(true);
    setDeleteFeedback({ type: "progress", message: requestElevation ? "Solicitando permisos de administrador..." : "Eliminando instancia..." });

    try {
      const response = await invoke<DeleteInstanceResponse>("delete_instance_with_elevation", {
        id: instanceToDelete.id,
        requestElevation,
      });

      if (response.status === "needs_elevation") {
        setDeleteFeedback({
          type: "error",
          message: "El sistema bloqueó la eliminación. Puedes solicitar permisos de administrador para completar el borrado.",
          needsElevation: true,
        });
        return;
      }

      if (response.status === "elevation_requested") {
        setDeleteFeedback({
          type: "progress",
          message: "Solicitud UAC enviada. Confirma el permiso para eliminar completamente los archivos protegidos.",
        });
        await reloadInstances();
        return;
      }

      setInstances((prev) => prev.filter((instance) => instance.id !== instanceToDelete.id));
      setSelectedInstance(null);
      setShowInstancePanel(false);
      setDeleteFeedback({ type: "success", message: "Instancia eliminada correctamente." });
      setTimeout(() => {
        setPendingDeleteInstance(null);
        setDeleteFeedback({ type: "idle", message: "" });
      }, 900);
      await reloadInstances();
    } catch (error) {
      const message = typeof error === "string" ? error : "No se pudo borrar la instancia.";
      setDeleteFeedback({ type: "error", message });
      setLaunchError(message);
    } finally {
      setDeleteInProgress(false);
    }
  };

  const handleInstanceAction = async (action: InstanceAction) => {
    if (!selectedInstance) return;
    if (action === "Editar") {
      enterEditMode();
      return;
    }
    if (action === "Iniciar") {
      await launchInstance();
      return;
    }
    if (action === "Forzar Cierre") {
      await invoke("force_close_instance", { id: selectedInstance.id });
      return;
    }
    if (action === "Carpeta") {
      await invoke("open_instance_folder", { id: selectedInstance.id });
      return;
    }
    if (action === "Copiar") {
      await copyInstance();
      return;
    }
    if (action === "Borrar") {
      setDeleteFeedback({ type: "idle", message: "" });
      setPendingDeleteInstance(selectedInstance);
    }
  };

  const goBackCreateSection = () => {
    const currentIndex = CREATE_SECTIONS.indexOf(activeCreateSection);
    if (currentIndex <= 0) return;
    setActiveCreateSection(CREATE_SECTIONS[currentIndex - 1]);
  };

  const goForwardCreateSection = () => {
    const currentIndex = CREATE_SECTIONS.indexOf(activeCreateSection);
    if (currentIndex >= CREATE_SECTIONS.length - 1) return;
    setActiveCreateSection(CREATE_SECTIONS[currentIndex + 1]);
  };

  const selectCreateSection = (section: (typeof CREATE_SECTIONS)[number]) => {
    setActiveCreateSection(section);
  };

  const launchInstance = async () => {
    if (!selectedInstance) return;
    setLaunchError(null);
    setLaunchLogs([]);
    setLaunchProgress({
      id: selectedInstance.id,
      value: 0,
      stage: "Solicitando lanzamiento",
      state: "running",
    });

    try {
      await invoke("launch_instance", { id: selectedInstance.id });
      setEditingInstance(selectedInstance);
      setActiveEditSection("Ejecucion");
      setShowInstancePanel(false);
    } catch (error) {
      const errorMessage = typeof error === "string" ? error : "No se pudo iniciar la instancia.";
      setLaunchError(errorMessage);
      setLaunchProgress({
        id: selectedInstance.id,
        value: 100,
        stage: "Error al iniciar",
        state: "error",
      });
    }
  };

  const filteredMinecraftVersions = useMemo(() => {
    if (minecraftFilter === "all") return minecraftVersions;
    return minecraftVersions.filter((entry) => ["release", "snapshot"].includes(entry.version_type));
  }, [minecraftFilter, minecraftVersions]);

  const createInstanceNow = async () => {
    if (!selectedMinecraftVersion || !selectedLoaderType) {
      setCreateError("Selecciona versión de Minecraft y loader.");
      return;
    }
    if (selectedLoaderType !== "vanilla" && !selectedLoaderVersion) {
      setCreateError("No hay versión de loader compatible.");
      return;
    }
    const name = newInstanceName.trim();
    if (!name) {
      setCreateError("Escribe un nombre de instancia.");
      return;
    }

    setCreateInProgress(true);
    setCreateError(null);
    setCreateProgress(null);
    setCreateLogs([]);
    try {
      const created = await invoke<InstanceInfo>("create_instance", {
        payload: {
          name,
          minecraftVersion: selectedMinecraftVersion,
          loaderType: selectedLoaderType,
          loaderVersion: selectedLoaderType === "vanilla" ? null : selectedLoaderVersion,
        },
      });
      await reloadInstances();
      setSelectedInstance(created);
      setShowInstancePanel(true);
      setAppMode("main");
      setActiveSection("instances");
      setNewInstanceName("");
    } catch (error) {
      setCreateError(typeof error === "string" ? error : "No se pudo crear la instancia.");
    } finally {
      setCreateInProgress(false);
    }
  };

  const shouldShowMinecraftBlock = ["Base", "Version"].includes(activeCreateSection);
  const shouldShowLoaderBlock = ["Base", "Loader"].includes(activeCreateSection);

  const onSelectInstance = (instance: InstanceInfo) => {
    setSelectedInstance(instance);
    setLaunchError(null);
    setLaunchLogs([]);
    setLaunchProgress(null);
    setShowInstancePanel(true);
  };

  const toggleExpandedCard = (instanceId: string) => {
    setExpandedInstanceId((prev) => (prev === instanceId ? null : instanceId));
  };

  const renderSectionPage = () => {
    if (activeSection !== "instances") {
      const label = SECTION_LABELS.find((section) => section.key === activeSection)?.label;
      return (
        <section className="full-section-page">
          <h1>{label}</h1>
          <p>Esta sección ahora ocupa una página completa. Aquí irá su contenido dedicado.</p>
        </section>
      );
    }

    return (
      <section className="full-section-page instances-page" onClick={() => setShowInstancePanel(false)}>
        <div className="instances-toolbar" onClick={(event) => event.stopPropagation()}>
          <div className="instances-toolbar-left">
            <button type="button" onClick={() => setAppMode("create")}>Crear instancia</button>
            <button type="button" onClick={() => setShowSearchInput((prev) => !prev)}>Buscar instancias</button>
            <div className="toolbar-menu" ref={sortMenuRef}>
              <button type="button" aria-label="Ordenar instancias" onClick={() => setShowSortMenu((prev) => !prev)}>Ordenar</button>
              {showSortMenu && (
                <div className="toolbar-dropdown" role="menu" aria-label="Ordenar instancias">
                  <button type="button" role="menuitem">Vista</button>
                  <button type="button" role="menuitem">Nombre</button>
                  <button type="button" role="menuitem">Fecha</button>
                </div>
              )}
            </div>
            <div className="toolbar-menu" ref={moreMenuRef}>
              <button type="button" aria-label="Mas acciones" onClick={() => setShowMoreMenu((prev) => !prev)}>Mas</button>
              {showMoreMenu && (
                <div className="toolbar-dropdown" role="menu" aria-label="Mas acciones">
                  <button type="button" role="menuitem">Importar</button>
                  <button type="button" role="menuitem" disabled>Futuro 1</button>
                  <button type="button" role="menuitem" disabled>Futuro 2</button>
                  <button type="button" role="menuitem" disabled>Futuro 3</button>
                  <button type="button" role="menuitem" disabled>Futuro 4</button>
                </div>
              )}
            </div>
            <label htmlFor="instances-search" className="sr-only">Buscar instancias</label>
            {showSearchInput && (
              <input
                ref={searchInputRef}
                id="instances-search"
                type="search"
                placeholder="Buscar instancias"
                value={instanceSearch}
                onChange={(event) => setInstanceSearch(event.target.value)}
              />
            )}
          </div>
        </div>

        <div className={`instances-workspace ${showInstancePanel && selectedInstance ? "with-panel" : ""}`}>
          <div className="instance-grid" onClick={(event) => event.stopPropagation()}>
            {instanceCards.map((instance) => {
              const tooltipText = `Version MC: ${instance.minecraft_version}\nLoader: ${prettyLoader(instance.loader_type)} ${instance.loader_version ?? "N/A"}\nAutor: Usuario Local\nPeso: ${formatBytes(instance.total_size_bytes)}`;
              return (
                <article
                  key={instance.id}
                  className={`instance-card ${selectedInstance?.id === instance.id ? "active" : ""} ${expandedInstanceId === instance.id ? "expanded" : ""}`}
                  onClick={() => onSelectInstance(instance)}
                >
                  <div className="instance-cover" aria-hidden="true">{instance.name.slice(0, 3).toUpperCase()}</div>
                  <div className="instance-meta">
                    <h3>{instance.name}</h3>
                    <div className="instance-extra-tooltip" tabIndex={0}>
                      ℹ️
                      <span className="tooltip-bubble">{tooltipText}</span>
                    </div>
                  </div>
                  <div className="instance-details">
                    <span className={`instance-state ${selectedInstance?.id === instance.id ? "online" : "idle"}`}>
                      {instance.state === "running" ? "En ejecución" : selectedInstance?.id === instance.id ? "Seleccionada" : "Disponible"}
                    </span>
                    <span>MC {instance.minecraft_version}</span>
                    <span>{prettyLoader(instance.loader_type)} {instance.loader_version ?? "Integrado"}</span>
                  </div>
                  <button
                    type="button"
                    className="instance-expand-btn"
                    onClick={(event) => {
                      event.stopPropagation();
                      toggleExpandedCard(instance.id);
                    }}
                    aria-expanded={expandedInstanceId === instance.id}
                  >
                    {expandedInstanceId === instance.id ? "Ocultar detalles" : "Expandir"}
                  </button>
                  <div className={`instance-expanded-content ${expandedInstanceId === instance.id ? "open" : ""}`}>
                    <p><strong>Tamaño:</strong> {formatBytes(instance.total_size_bytes)}</p>
                    <p><strong>ID:</strong> {instance.id}</p>
                    <p><strong>Compatibilidad:</strong> Perfil optimizado para escritorio Tauri.</p>
                  </div>
                </article>
              );
            })}
            {instanceCards.length === 0 && <p>No hay resultados para la búsqueda actual.</p>}
          </div>

          {showInstancePanel && selectedInstance && (
            <aside className="instance-right-panel" onClick={(event) => event.stopPropagation()}>
              <h3>{selectedInstance.name}</h3>
              <p className="instance-right-meta">{selectedInstance.minecraft_version} · {prettyLoader(selectedInstance.loader_type)}</p>
              <div className="instance-right-actions">
                {INSTANCE_ACTIONS.map((action) => (
                  <button
                    key={action}
                    type="button"
                    onClick={() => void handleInstanceAction(action)}
                  >
                    {action}
                  </button>
                ))}
              </div>
            </aside>
          )}
        </div>
      </section>
    );
  };

  if (appMode === "create") {
    return (
      <div className="app-shell">
        <header className="topbar-primary">
          <div className="topbar-left-controls">
            <button type="button" aria-label="Atras" className="arrow-button" onClick={goBackCreateSection}>←</button>
            <button type="button" aria-label="Adelante" className="arrow-button" onClick={goForwardCreateSection}>→</button>
            <div className="brand">Launcher Principal</div>
          </div>
          <div className="topbar-info">Creando nueva instancia</div>
        </header>
        <div className="create-layout create-layout-wide">
          <aside className="create-left-sidebar compact-sidebar">
            {CREATE_SECTIONS.map((section) => (
              <button
                key={section}
                type="button"
                className={activeCreateSection === section ? "active" : ""}
                onClick={() => selectCreateSection(section)}
              >
                {section}
              </button>
            ))}
          </aside>
          <main className="create-main-content create-main-grid">
            {shouldShowMinecraftBlock && (
            <section className="create-block">
              <header><h2>Bloque 1 · Versiones Minecraft</h2></header>
              <div className="create-block-body">
                <div className="create-list-wrap">
                  <table className="version-table">
                    <thead><tr><th>Version</th><th>Fecha de lanzado</th><th>Tipo</th></tr></thead>
                    <tbody>
                      {minecraftVersionsLoading && <tr><td colSpan={3}>Cargando versiones oficiales desde Mojang/Microsoft...</td></tr>}
                      {!minecraftVersionsLoading && minecraftVersionsError && <tr><td colSpan={3}>{minecraftVersionsError}</td></tr>}
                      {filteredMinecraftVersions.map((entry) => (
                        <tr key={entry.id} className={selectedMinecraftVersion === entry.id ? "selected" : ""} onClick={() => setSelectedMinecraftVersion(entry.id)}>
                          <td>{entry.id}</td>
                          <td>{new Date(entry.release_time).toLocaleDateString("es-ES")}</td>
                          <td>{entry.version_type}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
                <aside className="block-sidebar">
                  <h3>Filtro</h3>
                  {[["all","Todas"],["playable","Versiones jugables"]].map(([value, label]) => (
                    <button key={value} type="button" className={minecraftFilter === value ? "active" : ""} onClick={() => setMinecraftFilter(value as MinecraftVersionFilter)}>{label}</button>
                  ))}
                </aside>
              </div>
            </section>
            )}
            {shouldShowLoaderBlock && (
            <section className="create-block">
              <header><h2>Bloque 2 · Loaders</h2></header>
              <div className="create-block-body">
                <div className="create-list-wrap">
                  <table className="version-table">
                    <thead><tr><th>Version</th><th>Compatibilidad</th><th>Estado</th></tr></thead>
                    <tbody>
                      {selectedLoaderType === null ? <tr><td colSpan={3}>Selecciona un loader.</td></tr> : selectedLoaderType === "vanilla" ? <tr className="selected"><td>Integrado</td><td>{selectedMinecraftVersion ?? "-"}</td><td>Recomendado</td></tr> : loaderVersionsLoading ? <tr><td colSpan={3}>Cargando loaders oficiales...</td></tr> : loaderVersionsError ? <tr><td colSpan={3}>{loaderVersionsError}</td></tr> : loaderVersions.length === 0 ? <tr><td colSpan={3}>Sin versiones compatibles.</td></tr> : loaderVersions.map((entry, idx) => (
                        <tr key={entry.version} className={selectedLoaderVersion === entry.version ? "selected" : ""} onClick={() => setSelectedLoaderVersion(entry.version)}>
                          <td>{entry.version}</td><td>{selectedMinecraftVersion ?? "-"}</td><td>{entry.stable || idx === 0 ? "Recomendada / Más actual" : "Disponible"}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
                <aside className="block-sidebar">
                  <h3>Cargador de mods</h3>
                  {LOADER_CHOICES.map((loader) => (
                    <button key={loader.value} type="button" className={selectedLoaderType === loader.value ? "active" : ""} disabled={!loader.supported} onClick={() => loader.supported && setSelectedLoaderType(loader.value as LoaderType)}>
                      {loader.label}
                    </button>
                  ))}
                </aside>
              </div>
            </section>
            )}
          </main>
          <aside className="create-right-sidebar compact-sidebar">
            <h3>Crear instancia</h3>
            <div className="create-form-group">
              <label htmlFor="instance-name">Nombre</label>
              <input
                id="instance-name"
                type="text"
                className={newInstanceName.trim() ? "field-complete" : ""}
                value={newInstanceName}
                onChange={(event) => setNewInstanceName(event.target.value)}
                placeholder="Mi instancia"
              />
            </div>
            <div className="create-selection-summary">
              <p>MC: <strong>{selectedMinecraftVersion ?? "Sin seleccionar"}</strong></p>
              <p>Loader: <strong>{selectedLoaderType ? prettyLoader(selectedLoaderType) : "Sin seleccionar"}</strong></p>
              <p>Version loader: <strong>{selectedLoaderType === "vanilla" ? "Integrado" : (selectedLoaderVersion ?? "Sin seleccionar")}</strong></p>
            </div>
            {createProgress && (
              <div className={`create-status-banner ${createProgress.state}`}>
                <p>
                  Progreso: {createProgress.stage} ({createProgress.value}%)
                </p>
                <div className="create-progress-track" aria-hidden="true">
                  <span style={{ width: `${createProgress.value}%` }} />
                </div>
              </div>
            )}
            {createError && <p className="execution-error create-error-banner">{createError}</p>}
            {createLogs.length > 0 && (
              <div className="create-log-box" aria-live="polite">
                {createLogs.slice(-4).map((entry, index) => (
                  <p key={`${entry.id}-${index}`}>[{entry.level.toUpperCase()}] {entry.message}</p>
                ))}
              </div>
            )}
            <button type="button" onClick={() => void createInstanceNow()} disabled={createInProgress}>{createInProgress ? "Creando..." : "Crear instancia"}</button>
            <button type="button" onClick={() => setAppMode("main")}>Cancelar</button>
          </aside>
        </div>
      </div>
    );
  }


  if (editingInstance) {
    return (
      <div className="app-shell" onClick={() => setEditingInstance(null)}>
        <header className="topbar-primary">
          <div className="topbar-left-controls">
            <button type="button" aria-label="Atras" className="arrow-button">←</button>
            <button type="button" aria-label="Adelante" className="arrow-button">→</button>
            <div className="brand">Launcher Principal</div>
          </div>
          <div className="topbar-info">Editando: {editingInstance.name}</div>
        </header>
        <div className="edit-layout" onClick={(event) => event.stopPropagation()}>
          <aside className="edit-left-sidebar compact-sidebar">
            {EDIT_SECTIONS.map((section) => (
              <button
                key={section}
                type="button"
                className={activeEditSection === section ? "active" : ""}
                onClick={() => setActiveEditSection(section)}
              >
                {section}
              </button>
            ))}
          </aside>
          <main className="edit-main-content">
            <h2>{activeEditSection}</h2>
            {activeEditSection === "Ejecucion" ? (
              <div className="execution-log" ref={executionLogRef}>
                <div className="execution-actions">
                  <button type="button" onClick={() => void launchInstance()}>Iniciar</button>
                  {launchProgress && <span>{launchProgress.stage} ({launchProgress.value}%)</span>}
                </div>
                {launchError && <p className="execution-error">{launchError}</p>}
                {launchLogs.length === 0 ? (
                  <p>Sin logs todavía. Pulsa iniciar para lanzar la instancia real desde backend.</p>
                ) : (
                  launchLogs.map((log, index) => (
                    <p key={`${log.level}-${index}`}>[{log.level.toUpperCase()}] {log.message}</p>
                  ))
                )}
              </div>
            ) : activeEditSection === "Configuracion" ? (
              <section className="instance-settings-panel">
                <header className="instance-settings-header">
                  <h3>Configuracion de instancia</h3>
                  <div className="instance-settings-tabs" role="tablist" aria-label="Configuracion de instancia">
                    {INSTANCE_CONFIG_TABS.map((tab) => (
                      <button
                        key={tab}
                        type="button"
                        role="tab"
                        aria-selected={activeInstanceConfigTab === tab}
                        className={activeInstanceConfigTab === tab ? "active" : ""}
                        onClick={() => setActiveInstanceConfigTab(tab)}
                      >
                        {tab}
                      </button>
                    ))}
                  </div>
                </header>

                {activeInstanceConfigTab === "General" && (
                  <div className="settings-block-grid">
                    {[
                      "Game Windows",
                      "Console",
                      "Window",
                      "Global Datapa Packs",
                      "Game Time",
                      "Default Account",
                      "ModLoaders",
                    ].map((option) => (
                      <article key={option} className="settings-card">
                        <h4>{option}</h4>
                        <p>Opciones rapidas para configurar {option.toLowerCase()}.</p>
                      </article>
                    ))}
                  </div>
                )}

                {activeInstanceConfigTab === "Java" && (
                  <div className="settings-java-layout">
                    <article className="settings-card">
                      <h4>Instalacion de Java</h4>
                      <label htmlFor="java-path">Ruta ejecutable</label>
                      <div className="settings-inline-field">
                        <input id="java-path" type="text" value="C:/Program Files/Java/jdk-21/bin/javaw.exe" readOnly />
                        <button type="button">Detectar Javas</button>
                        <button type="button">Explorar</button>
                      </div>
                    </article>

                    <article className="settings-card">
                      <h4>Memoria</h4>
                      <p>Asignacion actual: 6144 MB de RAM.</p>
                      <input type="range" min={1024} max={16384} step={512} defaultValue={6144} />
                    </article>

                    <article className="settings-card">
                      <h4>Argumentos de Java</h4>
                      <textarea rows={5} defaultValue="-XX:+UseG1GC -XX:+UnlockExperimentalVMOptions" />
                    </article>
                  </div>
                )}

                {activeInstanceConfigTab !== "General" && activeInstanceConfigTab !== "Java" && (
                  <article className="settings-card settings-placeholder">
                    <h4>{activeInstanceConfigTab}</h4>
                    <p>Seccion lista para configurar opciones avanzadas de la instancia.</p>
                  </article>
                )}
              </section>
            ) : (
              <p>Vista completa de la instancia. Todo lo demás está oculto, excepto la barra superior principal.</p>
            )}
          </main>
          <aside className="edit-right-sidebar compact-sidebar">
            <h3>Acciones de {activeEditSection}</h3>
            <button type="button">Accion 1</button>
            <button type="button">Accion 2</button>
            <button type="button">Accion 3</button>
          </aside>
        </div>
      </div>
    );
  }

  return (
    <div className="app-shell" onClick={() => setShowInstancePanel(false)}>
      <header className="topbar-primary">
        <div className="topbar-left-controls">
          <button type="button" aria-label="Atras" className="arrow-button">←</button>
          <button type="button" aria-label="Adelante" className="arrow-button">→</button>
          <div className="brand">Launcher Principal</div>
        </div>
        <div className="topbar-right-controls">
          <div className="topbar-info" />
        </div>
      </header>

      <nav className="topbar-secondary" onClick={(event) => event.stopPropagation()}>
        <div className="secondary-nav-items">
          {SECTION_LABELS.map((section) => (
            <button
              key={section.key}
              type="button"
              className={activeSection === section.key ? "active" : ""}
              onClick={() => {
                setActiveSection(section.key);
                setShowInstancePanel(false);
              }}
            >
              {section.label}
            </button>
          ))}
        </div>

        <div className="topbar-profile" ref={profileMenuRef}>
          <button type="button" onClick={() => setShowProfileMenu((prev) => !prev)}>Perfil</button>
          {showProfileMenu && (
            <div className="profile-menu" role="menu" aria-label="Perfil">
              <button type="button" role="menuitem">Perfil 1</button>
              <button type="button" role="menuitem">Perfil 2</button>
              <button type="button" role="menuitem">Perfil 3</button>
              <button type="button" role="menuitem">Perfil 4</button>
              <button type="button" role="menuitem">Perfil 5</button>
            </div>
          )}
        </div>
      </nav>

      <main className="content-wrap">{renderSectionPage()}</main>

      {pendingDeleteInstance && (
        <div className="delete-modal-backdrop" onClick={() => !deleteInProgress && setPendingDeleteInstance(null)}>
          <div className="delete-modal" onClick={(event) => event.stopPropagation()}>
            <h3>Eliminar instancia</h3>
            <p>
              ¿Seguro que quieres eliminar <strong>{pendingDeleteInstance.name}</strong>? Esta accion borrara
              todos sus archivos y no se puede deshacer.
            </p>
            {deleteFeedback.type !== "idle" && (
              <p className={`delete-feedback ${deleteFeedback.type}`}>{deleteFeedback.message}</p>
            )}
            <div className="delete-modal-actions">
              <button type="button" disabled={deleteInProgress} onClick={() => setPendingDeleteInstance(null)}>Cancelar</button>
              {deleteFeedback.needsElevation && (
                <button type="button" className="warning" disabled={deleteInProgress} onClick={() => void deleteInstanceById(pendingDeleteInstance, true)}>
                  Solicitar permisos de administrador
                </button>
              )}
              <button type="button" className="danger" disabled={deleteInProgress} onClick={() => void deleteInstanceById(pendingDeleteInstance)}>
                {deleteInProgress ? "Procesando..." : "Eliminar instancia"}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

export default App;
