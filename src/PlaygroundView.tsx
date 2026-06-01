import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { humanizeError } from "./errors";
import { t } from "./i18n";

interface ApiKey {
  id: string;
  secret: string;
  label: string;
  enabled: boolean;
  weight: number;
}

interface ProxyInfo {
  local_base: string;
  upstream: string;
  token: string;
  pool_enabled: boolean;
  primary_key_id: string | null;
}

interface ModelInfo {
  id: string;
  owned_by: string | null;
}

interface ChatMessage {
  id: string;
  role: "user" | "assistant" | "system";
  content: string;
  imageUrl?: string;
  imageUrls?: string[];
  timestamp: number;
  model?: string;
  tokens?: { input: number; output: number };
}

interface ImageUpload {
  id: string;
  name: string;
  type: string;
  size: number;
  dataUrl: string;
}

interface PlaygroundSession {
  id: string;
  title: string;
  mode: "chat" | "image";
  messages: ChatMessage[];
  timestamp: number;
  systemPrompt: string;
  temperature: number;
  maxTokens: number;
  selectedModel: string;
  selectedKeyId: string;
  imageSize: string;
  imageQuality: string;
}

const IMAGE_SIZE_OPTIONS = [
  { value: "1024x1024", labelKey: "playground.imageSizeSquare" },
  { value: "1792x1024", labelKey: "playground.imageSizeLandscapeWide" },
  { value: "1024x768", labelKey: "playground.imageSizeLandscape" },
  { value: "768x1024", labelKey: "playground.imageSizePortrait" },
  { value: "1024x1792", labelKey: "playground.imageSizePortraitTall" },
  { value: "", labelKey: "playground.imageSizeAuto" },
];

function normalizeImageSize(size: string | undefined): string {
  if (!size) return "";
  if (IMAGE_SIZE_OPTIONS.some((option) => option.value === size)) return size;
  if (["256x256", "512x512"].includes(size)) return "1024x1024";
  return "";
}

function msgId() {
  return Date.now().toString(36) + Math.random().toString(36).slice(2, 8);
}

function isDefaultTitle(title: string) {
  return (
    title === "新建对话" ||
    title === "New Chat" ||
    title === "新建生图" ||
    title === "New Image" ||
    title === t("playground.newChat") ||
    title === t("playground.newImage") ||
    !title
  );
}

function cleanModelOutput(text: string): string {
  if (!text) return "";
  return text
    .replace(/entity\[[^\]]*\]/g, "")
    .replace(/citeturn\d+search\d+/g, "")
    .replace(/citeturn\d+/g, "")
    .replace(/turn\d+search\d+/g, "")
    .replace(/turn\d+/g, "");
}

export default function PlaygroundView() {
  const [keys, setKeys] = useState<ApiKey[]>([]);
  const [proxyInfo, setProxyInfo] = useState<ProxyInfo | null>(null);
  const [modelsCache, setModelsCache] = useState<Record<string, ModelInfo[]>>({});
  const [loadingModels, setLoadingModels] = useState(false);
  const [input, setInput] = useState("");
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [savingImages, setSavingImages] = useState<Record<string, boolean>>({});
  const [previewImageUrl, setPreviewImageUrl] = useState<string | null>(null);
  const [uploadedImages, setUploadedImages] = useState<ImageUpload[]>([]);

  const [sessions, setSessions] = useState<PlaygroundSession[]>([]);
  const [activeSessionId, setActiveSessionId] = useState<string>("");

  const messagesEndRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const activeSession = sessions.find((s) => s.id === activeSessionId);

  const isImageModel = (id: string) => {
    const lower = id.toLowerCase();
    return (
      lower.includes("dall") ||
      lower.includes("diff") ||
      lower.includes("flux") ||
      lower.includes("midj") ||
      lower.includes("mj") ||
      lower.includes("image") ||
      lower.includes("cog") ||
      lower.includes("sd") ||
      lower.includes("paint") ||
      lower.includes("draw") ||
      lower.includes("art") ||
      lower.includes("pic") ||
      lower.includes("pixel") ||
      lower.includes("photo")
    );
  };

  const models = activeSession ? (modelsCache[activeSession.selectedKeyId] || []) : [];

  const chatModels = models.filter((m) => !isImageModel(m.id));

  const imageModels = models.filter((m) => isImageModel(m.id));

  const displayedModels = activeSession?.mode === "image" ? imageModels : chatModels;

  const isModelValid = !!(
    keys.length > 0 &&
    activeSession &&
    activeSession.selectedModel &&
    displayedModels.some((m) => m.id === activeSession.selectedModel)
  );

  useEffect(() => {
    loadKeysAndSessions();
  }, []);

  useEffect(() => {
    if (activeSession?.messages) {
      messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
    }
  }, [activeSession?.messages?.length]);

  async function loadKeysAndSessions() {
    try {
      const info = await invoke<ProxyInfo>("proxy_info");
      setProxyInfo(info);
      const pool = await invoke<{
        keys: ApiKey[];
        pool_enabled: boolean;
        primary_key_id: string | null;
      }>("load_pool");
      const enabledKeys = pool.keys.filter((k) => k.enabled);
      setKeys(enabledKeys);

      // Load sessions from localStorage
      const saved = localStorage.getItem("beeapi-playground-sessions");
      let loadedSessions: PlaygroundSession[] = [];
      if (saved) {
        try {
          loadedSessions = JSON.parse(saved);
        } catch (e) {
          console.error("Failed to parse saved sessions", e);
        }
      }

      // Default selected key
      let defaultKeyId = "";
      if (enabledKeys.length > 0) {
        const primary = enabledKeys.find((k) => k.id === pool.primary_key_id);
        defaultKeyId = primary ? primary.id : enabledKeys[0].id;
      }

      if (enabledKeys.length === 0) {
        loadedSessions = loadedSessions.map((s) => ({ ...s, selectedModel: "", selectedKeyId: "" }));
      }

      // If no sessions, create a default one
      if (loadedSessions.length === 0) {
        const defaultSession: PlaygroundSession = {
          id: msgId(),
          title: t("playground.newChat"),
          mode: "chat",
          messages: [],
          timestamp: Date.now(),
          systemPrompt: "",
          temperature: 0.7,
          maxTokens: 4096,
          selectedModel: "",
          selectedKeyId: defaultKeyId,
          imageSize: "1024x1024",
          imageQuality: "standard",
        };
        loadedSessions = [defaultSession];
        localStorage.setItem("beeapi-playground-sessions", JSON.stringify(loadedSessions));
      }

      loadedSessions = loadedSessions.map((s) => ({
        ...s,
        imageSize: normalizeImageSize(s.imageSize),
      }));

      setSessions(loadedSessions);

      // Restore active session ID
      const savedActiveId = localStorage.getItem("beeapi-playground-active-id");
      const activeExists = loadedSessions.some((s) => s.id === savedActiveId);
      const targetActiveId = activeExists ? savedActiveId! : loadedSessions[0].id;
      setActiveSessionId(targetActiveId);
      localStorage.setItem("beeapi-playground-active-id", targetActiveId);

      // Trigger model loading if a session had a selected model, or let it load
      // We can also fetch models using the target session's selected key
      const activeSessionObj = loadedSessions.find((s) => s.id === targetActiveId);
      if (activeSessionObj && activeSessionObj.selectedKeyId) {
        fetchModelsForId(activeSessionObj.selectedKeyId, targetActiveId);
      }
    } catch (e) {
      setError(humanizeError(String(e)));
    }
  }

  function saveSessionsList(list: PlaygroundSession[]) {
    setSessions(list);
    localStorage.setItem("beeapi-playground-sessions", JSON.stringify(list));
  }

  function updateActiveSession(updater: (s: PlaygroundSession) => PlaygroundSession) {
    if (!activeSessionId) return;
    setSessions((prevSessions) => {
      const nextList = prevSessions.map((s) => {
        if (s.id === activeSessionId) {
          return updater(s);
        }
        return s;
      });
      localStorage.setItem("beeapi-playground-sessions", JSON.stringify(nextList));
      return nextList;
    });
  }

  async function selectKeyForSession(targetKeyId: string, sessionId: string) {
    if (!targetKeyId || !sessionId) return;
    
    const cached = modelsCache[targetKeyId];
    if (cached && cached.length > 0) {
      updateActiveSession((s) => {
        // Find appropriate default model from cached list
        let targetModel = "";
        if (s.mode === "image") {
          const imgModel = cached.find((m) => isImageModel(m.id));
          targetModel = imgModel ? imgModel.id : "";
        } else {
          const chatModel = cached.find(
            (m) =>
              m.id.includes("gpt") ||
              m.id.includes("claude") ||
              m.id.includes("gemini"),
          );
          targetModel = chatModel ? chatModel.id : cached[0].id;
        }
        return {
          ...s,
          selectedKeyId: targetKeyId,
          selectedModel: targetModel,
        };
      });
      setError(null);
    } else {
      updateActiveSession((s) => ({ ...s, selectedKeyId: targetKeyId, selectedModel: "" }));
      await fetchModelsForId(targetKeyId, sessionId, false);
    }
  }

  function setMode(mode: "chat" | "image") {
    if (mode !== "image") {
      setUploadedImages([]);
    }
    updateActiveSession((s) => {
      let nextModel = s.selectedModel;
      
      if (mode === "image") {
        if (!isImageModel(nextModel)) {
          const firstImg = imageModels[0]?.id || "";
          nextModel = firstImg;
        }
      } else {
        if (isImageModel(nextModel) || !nextModel) {
          const chatModel = chatModels.find(
            (m) =>
              m.id.includes("gpt") ||
              m.id.includes("claude") ||
              m.id.includes("gemini"),
          );
          nextModel = chatModel ? chatModel.id : (chatModels[0]?.id || "");
        }
      }

      let nextTitle = s.title;
      if (isDefaultTitle(s.title)) {
        nextTitle = mode === "image" ? t("playground.newImage") : t("playground.newChat");
      }

      return {
        ...s,
        mode,
        selectedModel: nextModel,
        title: nextTitle,
      };
    });
  }

  function onNewSession() {
    let defaultKeyId = "";
    if (keys.length > 0) {
      const primary = keys.find((k) => proxyInfo && k.id === proxyInfo.primary_key_id);
      defaultKeyId = primary ? primary.id : keys[0].id;
    }

    const cached = modelsCache[defaultKeyId] || [];
    let defaultModel = "";
    if (cached.length > 0) {
      const chatModel = cached.find(
        (m) =>
          m.id.includes("gpt") ||
          m.id.includes("claude") ||
          m.id.includes("gemini"),
      );
      defaultModel = chatModel ? chatModel.id : cached[0].id;
    }

    const newSession: PlaygroundSession = {
      id: msgId(),
      title: t("playground.newChat"),
      mode: "chat",
      messages: [],
      timestamp: Date.now(),
      systemPrompt: "",
      temperature: 0.7,
      maxTokens: 4096,
      selectedModel: defaultModel,
      selectedKeyId: defaultKeyId,
      imageSize: "1024x1024",
      imageQuality: "standard",
    };

    const nextList = [newSession, ...sessions];
    saveSessionsList(nextList);
    setActiveSessionId(newSession.id);
    localStorage.setItem("beeapi-playground-active-id", newSession.id);
    setError(null);

    if (cached.length === 0 && defaultKeyId) {
      fetchModelsForId(defaultKeyId, newSession.id, true);
    }
  }

  function onDeleteSession(e: React.MouseEvent, id: string) {
    e.stopPropagation();
    const nextList = sessions.filter((s) => s.id !== id);
    
    if (nextList.length === 0) {
      // If we deleted the last session, create a new one
      let defaultKeyId = "";
      if (keys.length > 0) {
        const primary = keys.find((k) => proxyInfo && k.id === proxyInfo.primary_key_id);
        defaultKeyId = primary ? primary.id : keys[0].id;
      }
      
      const cached = modelsCache[defaultKeyId] || [];
      let defaultModel = "";
      if (cached.length > 0) {
        const chatModel = cached.find(
          (m) =>
            m.id.includes("gpt") ||
            m.id.includes("claude") ||
            m.id.includes("gemini"),
        );
        defaultModel = chatModel ? chatModel.id : cached[0].id;
      }

      const newSession: PlaygroundSession = {
        id: msgId(),
        title: t("playground.newChat"),
        mode: "chat",
        messages: [],
        timestamp: Date.now(),
        systemPrompt: "",
        temperature: 0.7,
        maxTokens: 4096,
        selectedModel: defaultModel,
        selectedKeyId: defaultKeyId,
        imageSize: "1024x1024",
        imageQuality: "standard",
      };
      saveSessionsList([newSession]);
      setActiveSessionId(newSession.id);
      localStorage.setItem("beeapi-playground-active-id", newSession.id);

      if (cached.length === 0 && defaultKeyId) {
        fetchModelsForId(defaultKeyId, newSession.id, true);
      }
    } else {
      saveSessionsList(nextList);
      if (activeSessionId === id) {
        setActiveSessionId(nextList[0].id);
        localStorage.setItem("beeapi-playground-active-id", nextList[0].id);
      }
    }
    setError(null);
  }

  function onSelectSession(id: string) {
    setActiveSessionId(id);
    localStorage.setItem("beeapi-playground-active-id", id);
    setError(null);
    
    const sessionObj = sessions.find((s) => s.id === id);
    if (sessionObj && sessionObj.selectedKeyId) {
      const cached = modelsCache[sessionObj.selectedKeyId];
      if (!cached || cached.length === 0) {
        fetchModelsForId(sessionObj.selectedKeyId, id, true);
      }
    }
  }

  async function fetchModelsForId(keyId: string, targetSessionId?: string, preserveSelection = false) {
    if (!keyId) return;
    const sessionId = targetSessionId || activeSessionId;
    setLoadingModels(true);
    setError(null);
    
    if (sessionId && !preserveSelection) {
      setSessions((prev) => {
        const next = prev.map((s) => (s.id === sessionId ? { ...s, selectedModel: "" } : s));
        localStorage.setItem("beeapi-playground-sessions", JSON.stringify(next));
        return next;
      });
    }

    try {
      const list = await invoke<ModelInfo[]>("fetch_models", { keyId });
      setModelsCache((prev) => ({ ...prev, [keyId]: list }));
      
      if (list.length === 0) {
        setError("该密钥无可用模型");
      }

      if (sessionId) {
        setSessions((prev) => {
          const sess = prev.find((s) => s.id === sessionId);
          if (!sess) return prev;
          
          if (preserveSelection && sess.selectedModel && list.some(m => m.id === sess.selectedModel)) {
            return prev;
          }

          const mode = sess.mode;
          
          let targetModel = "";
          if (mode === "image") {
            const imgModel = list.find((m) => isImageModel(m.id));
            targetModel = imgModel ? imgModel.id : "";
          } else {
            const chatModel = list.find(
              (m) =>
                m.id.includes("gpt") ||
                m.id.includes("claude") ||
                m.id.includes("gemini"),
            );
            targetModel = chatModel ? chatModel.id : (list[0]?.id || "");
          }

          const next = prev.map((s) => (s.id === sessionId ? { ...s, selectedModel: targetModel } : s));
          localStorage.setItem("beeapi-playground-sessions", JSON.stringify(next));
          return next;
        });
      }
    } catch (e) {
      const errStr = humanizeError(String(e));
      setError(errStr);
      setModelsCache((prev) => {
        const next = { ...prev };
        delete next[keyId];
        return next;
      });
      if (sessionId) {
        setSessions((prev) => {
          const next = prev.map((s) => (s.id === sessionId ? { ...s, selectedModel: "" } : s));
          localStorage.setItem("beeapi-playground-sessions", JSON.stringify(next));
          return next;
        });
      }
    } finally {
      setLoadingModels(false);
    }
  }

  async function onFetchModels() {
    if (!activeSession) return;
    setModelsCache((prev) => {
      const next = { ...prev };
      delete next[activeSession.selectedKeyId];
      return next;
    });
    await fetchModelsForId(activeSession.selectedKeyId, activeSession.id, false);
  }

  function getApiBase(): string {
    if (!proxyInfo) return "";
    return proxyInfo.local_base;
  }

  function getApiToken(): string {
    if (!proxyInfo) return "";
    return proxyInfo.token;
  }

  function fileToDataUrl(file: File): Promise<string> {
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => resolve(String(reader.result || ""));
      reader.onerror = () => reject(reader.error || new Error("Failed to read file"));
      reader.readAsDataURL(file);
    });
  }

  function dataUrlToFile(dataUrl: string, name: string, fallbackType = "image/png"): File {
    const commaIdx = dataUrl.indexOf(",");
    if (commaIdx === -1) throw new Error("Invalid data URL");
    const meta = dataUrl.slice(0, commaIdx);
    const mime = meta.match(/^data:([^;]+)/)?.[1] || fallbackType;
    const binaryStr = window.atob(dataUrl.slice(commaIdx + 1));
    const bytes = new Uint8Array(binaryStr.length);
    for (let i = 0; i < binaryStr.length; i++) {
      bytes[i] = binaryStr.charCodeAt(i);
    }
    return new File([bytes], name || `image-${Date.now()}.png`, { type: mime });
  }

  async function onUploadImages(e: React.ChangeEvent<HTMLInputElement>) {
    const files = Array.from(e.target.files || []).filter((file) => file.type.startsWith("image/"));
    e.target.value = "";
    if (files.length === 0) return;

    try {
      const images = await Promise.all(
        files.map(async (file) => ({
          id: msgId(),
          name: file.name,
          type: file.type || "image/png",
          size: file.size,
          dataUrl: await fileToDataUrl(file),
        })),
      );
      setUploadedImages((prev) => [...prev, ...images]);
      setError(null);
    } catch (e) {
      setError(humanizeError(String(e)));
    }
  }

  function removeUploadedImage(id: string) {
    setUploadedImages((prev) => prev.filter((img) => img.id !== id));
  }

  async function onSend() {
    if (!input.trim() || sending || !activeSession) return;
    const token = getApiToken();
    if (!token) {
      setError(t("playground.noKey"));
      return;
    }

    const attachedImages = activeSession.mode === "image" ? uploadedImages : [];

    const userMsg: ChatMessage = {
      id: msgId(),
      role: "user",
      content: input.trim(),
      imageUrls: attachedImages.map((img) => img.dataUrl),
      timestamp: Date.now(),
    };

    const updatedMessages = [...activeSession.messages, userMsg];
    
    // Auto update title if first message
    let updatedTitle = activeSession.title;
    if (isDefaultTitle(activeSession.title) && activeSession.messages.length === 0) {
      updatedTitle = userMsg.content.slice(0, 16);
      if (userMsg.content.length > 16) updatedTitle += "...";
    }

    updateActiveSession((s) => ({
      ...s,
      messages: updatedMessages,
      title: updatedTitle,
    }));
    setInput("");
    if (activeSession.mode === "image") {
      setUploadedImages([]);
    }
    setSending(true);
    setError(null);

    try {
      if (activeSession.mode === "image") {
        await sendImageRequest(userMsg, updatedMessages, attachedImages);
      } else {
        await sendChatRequest(userMsg, updatedMessages);
      }
    } catch (e) {
      if (activeSession.mode === "image" && attachedImages.length > 0) {
        setUploadedImages(attachedImages);
      }
      setError(humanizeError(String(e)));
    } finally {
      setSending(false);
      inputRef.current?.focus();
    }
  }

  async function sendChatRequest(userMsg: ChatMessage, currentMessages: ChatMessage[]) {
    if (!activeSession) return;
    const base = getApiBase();
    const token = getApiToken();
    const root = base.replace(/\/+$/, "");
    const url = root.endsWith("/v1")
      ? `${root}/chat/completions`
      : `${root}/v1/chat/completions`;

    const chatHistory = currentMessages
      .slice(0, -1)
      .filter((m) => m.role === "user" || m.role === "assistant")
      .map((m) => ({ role: m.role, content: m.content }));

    const allMessages = [];
    if (activeSession.systemPrompt.trim()) {
      allMessages.push({ role: "system", content: activeSession.systemPrompt.trim() });
    }
    allMessages.push(...chatHistory);
    allMessages.push({ role: "user", content: userMsg.content });

    const body: Record<string, unknown> = {
      model: activeSession.selectedModel,
      messages: allMessages,
      temperature: activeSession.temperature,
      max_tokens: activeSession.maxTokens,
      stream: false,
    };

    const resp = await fetch(url, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${token}`,
        "X-Use-Key-Id": activeSession.selectedKeyId,
      },
      body: JSON.stringify(body),
    });

    if (!resp.ok) {
      const errText = await resp.text().catch(() => "");
      throw new Error(
        `${resp.status}: ${errText.slice(0, 300) || resp.statusText}`,
      );
    }

    const data = await resp.json();
    const choice = data.choices?.[0];
    const content = cleanModelOutput(choice?.message?.content || t("playground.noResponse"));
    const usage = data.usage;

    const assistantMsg: ChatMessage = {
      id: msgId(),
      role: "assistant",
      content,
      timestamp: Date.now(),
      model: data.model || activeSession.selectedModel,
      tokens: usage
        ? {
            input: usage.prompt_tokens || 0,
            output: usage.completion_tokens || 0,
          }
        : undefined,
    };

    updateActiveSession((s) => ({
      ...s,
      messages: [...currentMessages, assistantMsg],
    }));
  }

  async function sendImageRequest(
    userMsg: ChatMessage,
    currentMessages: ChatMessage[],
    referenceImages: ImageUpload[] = [],
  ) {
    if (!activeSession) return;
    const base = getApiBase();
    const token = getApiToken();
    const root = base.replace(/\/+$/, "");
    const endpoint = referenceImages.length > 0 ? "edits" : "generations";
    const url = root.endsWith("/v1")
      ? `${root}/images/${endpoint}`
      : `${root}/v1/images/${endpoint}`;

    let resp: Response;
    if (referenceImages.length > 0) {
      const form = new FormData();
      form.append("model", activeSession.selectedModel);
      form.append("prompt", userMsg.content);
      form.append("n", "1");
      form.append("quality", activeSession.imageQuality);
      if (activeSession.imageSize) {
        form.append("size", activeSession.imageSize);
      }
      referenceImages.forEach((img) => {
        form.append("image", dataUrlToFile(img.dataUrl, img.name, img.type));
      });

      resp = await fetch(url, {
        method: "POST",
        headers: {
          Authorization: `Bearer ${token}`,
          "X-Use-Key-Id": activeSession.selectedKeyId,
        },
        body: form,
      });
    } else {
      const body: Record<string, unknown> = {
        model: activeSession.selectedModel,
        prompt: userMsg.content,
        n: 1,
        quality: activeSession.imageQuality,
      };
      if (activeSession.imageSize) {
        body.size = activeSession.imageSize;
      }

      resp = await fetch(url, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Authorization: `Bearer ${token}`,
          "X-Use-Key-Id": activeSession.selectedKeyId,
        },
        body: JSON.stringify(body),
      });
    }

    if (!resp.ok) {
      const errText = await resp.text().catch(() => "");
      throw new Error(
        `${resp.status}: ${errText.slice(0, 300) || resp.statusText}`,
      );
    }

    const data = await resp.json();
    const imgData = data.data?.[0];
    const imgUrl = imgData?.url || imgData?.b64_json;
    const revisedPrompt = imgData?.revised_prompt;

    const assistantMsg: ChatMessage = {
      id: msgId(),
      role: "assistant",
      content: revisedPrompt || t("playground.imageGenerated"),
      imageUrl: imgUrl?.startsWith("http")
        ? imgUrl
        : imgUrl
          ? `data:image/png;base64,${imgUrl}`
          : undefined,
      timestamp: Date.now(),
      model: data.model || activeSession.selectedModel,
    };

    updateActiveSession((s) => ({
      ...s,
      messages: [...currentMessages, assistantMsg],
    }));
  }

  function onClear() {
    updateActiveSession((s) => ({
      ...s,
      messages: [],
    }));
    setError(null);
  }

  async function saveImage(imageUrl: string) {
    try {
      const { save } = await import("@tauri-apps/plugin-dialog");
      const fileName = `image-${Date.now()}.png`;
      const filePath = await save({
        filters: [{
          name: "Image",
          extensions: ["png", "jpg", "jpeg", "webp"]
        }],
        defaultPath: fileName
      });
      
      if (!filePath) return;

      setSavingImages(prev => ({ ...prev, [imageUrl]: true }));
      setError(null);

      if (imageUrl.startsWith("data:")) {
        const commaIdx = imageUrl.indexOf(",");
        if (commaIdx === -1) throw new Error("Invalid data URL");
        const b64Data = imageUrl.slice(commaIdx + 1);
        const binaryStr = window.atob(b64Data);
        const bytes = new Uint8Array(binaryStr.length);
        for (let i = 0; i < binaryStr.length; i++) {
          bytes[i] = binaryStr.charCodeAt(i);
        }
        const { writeFile } = await import("@tauri-apps/plugin-fs");
        await writeFile(filePath, bytes);
      } else {
        await invoke("download_image_to", { url: imageUrl, path: filePath });
      }
    } catch (e) {
      console.error("Failed to save image", e);
      setError(t("playground.saveFailed", { error: String(e) }));
    } finally {
      setSavingImages(prev => ({ ...prev, [imageUrl]: false }));
    }
  }

  function onKeyDown(e: React.KeyboardEvent<HTMLTextAreaElement>) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      onSend();
    }
  }

  function formatTime(ts: number): string {
    const d = new Date(ts);
    const pad = (n: number) => String(n).padStart(2, "0");
    return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
  }

  const activeKeyLabel =
    activeSession && keys.find((k) => k.id === activeSession.selectedKeyId)?.label || "—";

  return (
    <div className="panel view-enter playground-view">
      {/* Left Sidebar: history */}
      <div className="playground-sidebar">
        <div className="playground-sidebar-header">
          <button className="btn primary" onClick={onNewSession} style={{ width: "100%", gap: 6, justifyContent: "center" }}>
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round">
              <line x1="12" y1="5" x2="12" y2="19" />
              <line x1="5" y1="12" x2="19" y2="12" />
            </svg>
            {t("playground.newChat")}
          </button>
        </div>
        
        <div className="playground-sidebar-list">
          {sessions.map((s) => (
            <div
              key={s.id}
              className={`playground-session-item ${s.id === activeSessionId ? "active" : ""}`}
              onClick={() => onSelectSession(s.id)}
            >
              <div className="playground-session-title-wrap">
                {s.mode === "chat" ? (
                  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" style={{ flexShrink: 0 }}>
                    <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
                  </svg>
                ) : (
                  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" style={{ flexShrink: 0 }}>
                    <rect x="3" y="3" width="18" height="18" rx="2" ry="2" />
                    <circle cx="8.5" cy="8.5" r="1.5" />
                    <path d="M21 15l-5-5L5 21" />
                  </svg>
                )}
                <span className="playground-session-title">{s.title}</span>
              </div>
              <div
                className="playground-session-delete"
                onClick={(e) => onDeleteSession(e, s.id)}
                title={t("keys.delete")}
              >
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                  <polyline points="3 6 5 6 21 6" />
                  <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
                </svg>
              </div>
            </div>
          ))}
        </div>
      </div>

      {/* Right Main area */}
      <div className="playground-main">
        {activeSession && (
          <>
            <header className="panel-header">
              <div className="big-mark svg-mark">
                <svg
                  width="18"
                  height="18"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.8"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                >
                  <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
                  <circle cx="9" cy="10" r="1" fill="currentColor" stroke="none" />
                  <circle cx="12" cy="10" r="1" fill="currentColor" stroke="none" />
                  <circle cx="15" cy="10" r="1" fill="currentColor" stroke="none" />
                </svg>
              </div>
              <div style={{ flex: 1 }}>
                <h2>
                  {t("playground.title")}
                  <span className={`pill ${activeSession.mode === "chat" ? "ok" : ""}`}>
                    {activeSession.mode === "chat"
                      ? t("playground.chatMode")
                      : t("playground.imageMode")}
                  </span>
                </h2>
                <div className="sub">{t("playground.desc")}</div>
              </div>
            </header>

            <div className="panel-divider" />

            {/* Toolbar */}
            <div className="playground-toolbar">
              {/* Mode selector */}
              <div className="segmented">
                <button
                  className={`segmented-btn ${activeSession.mode === "chat" ? "active" : ""}`}
                  onClick={() => setMode("chat")}
                >
                  {t("playground.chatMode")}
                </button>
                <button
                  className={`segmented-btn ${activeSession.mode === "image" ? "active" : ""}`}
                  onClick={() => setMode("image")}
                >
                  {t("playground.imageMode")}
                </button>
              </div>

              {/* Key selector */}
              <div className="playground-key-select">
                <svg
                  width="12"
                  height="12"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="2"
                  style={{ flexShrink: 0 }}
                >
                  <circle cx="8" cy="14" r="4" />
                  <path d="M11 14 L20 14 L20 17 M17 14 L17 17" />
                </svg>
                <select
                  className="select"
                  value={activeSession.selectedKeyId}
                  onChange={(e) => {
                    selectKeyForSession(e.target.value, activeSession.id);
                  }}
                >
                  {keys.length === 0 ? (
                    <option value="">{t("playground.noKeysAvailable")}</option>
                  ) : (
                    keys.map((k) => (
                      <option key={k.id} value={k.id}>
                        {k.label} (...{k.secret.slice(-4)})
                      </option>
                    ))
                  )}
                </select>
              </div>

              {/* Model selector */}
              <div className="playground-model-select">
                <select
                  className="select"
                  value={activeSession.selectedModel}
                  onChange={(e) => updateActiveSession((s) => ({ ...s, selectedModel: e.target.value }))}
                  disabled={loadingModels || displayedModels.length === 0}
                >
                  {(!activeSession.selectedModel || !displayedModels.some((m) => m.id === activeSession.selectedModel)) && (
                    <option value="">
                      {loadingModels ? t("playground.fetching") : (displayedModels.length === 0 ? t("playground.noModelsAvailable") : t("playground.selectModel"))}
                    </option>
                  )}
                  {displayedModels.map((m) => (
                    <option key={m.id} value={m.id}>
                      {m.id}
                    </option>
                  ))}
                </select>
                <button
                  className="btn small"
                  onClick={onFetchModels}
                  disabled={loadingModels}
                >
                  {loadingModels ? (
                    <span className="spinner" />
                  ) : (
                    t("playground.fetchModels")
                  )}
                </button>
              </div>

              <div className="spacer" />

              {/* Settings toggle */}
              <button
                className={`btn small ${showSettings ? "primary" : "ghost"}`}
                onClick={() => setShowSettings(!showSettings)}
                title={t("playground.settings")}
              >
                <svg
                  width="13"
                  height="13"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.8"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                >
                  <circle cx="12" cy="12" r="3" />
                  <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
                </svg>
                {t("playground.settings")}
              </button>

              <button className="btn small danger" onClick={onClear}>
                {t("playground.clear")}
              </button>
            </div>

            {/* Settings panel */}
            {showSettings && (
              <div className="playground-settings toast-in">
                {activeSession.mode === "chat" && (
                  <>
                    <div className="playground-setting-row">
                      <label>{t("playground.systemPrompt")}</label>
                      <textarea
                        className="input"
                        value={activeSession.systemPrompt}
                        onChange={(e) => updateActiveSession((s) => ({ ...s, systemPrompt: e.target.value }))}
                        placeholder={t("playground.systemPromptPlaceholder")}
                        rows={2}
                        style={{ resize: "vertical", minHeight: 48 }}
                      />
                    </div>
                    <div className="playground-setting-row">
                      <label>{t("playground.temperature")}</label>
                      <div className="playground-slider-row">
                        <input
                          type="range"
                          min="0"
                          max="2"
                          step="0.1"
                          value={activeSession.temperature}
                          onChange={(e) => updateActiveSession((s) => ({ ...s, temperature: parseFloat(e.target.value) }))}
                          className="playground-slider"
                        />
                        <span className="playground-slider-value">{activeSession.temperature}</span>
                      </div>
                    </div>
                    <div className="playground-setting-row">
                      <label>{t("playground.maxTokens")}</label>
                      <input
                        className="input"
                        type="number"
                        min={1}
                        max={128000}
                        value={activeSession.maxTokens}
                        onChange={(e) =>
                          updateActiveSession((s) => ({ ...s, maxTokens: parseInt(e.target.value) || 4096 }))
                        }
                        style={{ width: 120 }}
                      />
                    </div>
                  </>
                )}
                {activeSession.mode === "image" && (
                  <>
                    <div className="playground-setting-row">
                      <label>{t("playground.imageSize")}</label>
                      <select
                        className="select"
                        value={activeSession.imageSize}
                        onChange={(e) => updateActiveSession((s) => ({ ...s, imageSize: e.target.value }))}
                        style={{ width: 160 }}
                      >
                        {IMAGE_SIZE_OPTIONS.map((option) => (
                          <option key={option.labelKey} value={option.value}>
                            {t(option.labelKey)}
                          </option>
                        ))}
                      </select>
                    </div>
                    <div className="playground-setting-row">
                      <label>{t("playground.imageQuality")}</label>
                      <div className="segmented">
                        <button
                          className={`segmented-btn ${activeSession.imageQuality === "standard" ? "active" : ""}`}
                          onClick={() => updateActiveSession((s) => ({ ...s, imageQuality: "standard" }))}
                        >
                          {t("playground.standard")}
                        </button>
                        <button
                          className={`segmented-btn ${activeSession.imageQuality === "hd" ? "active" : ""}`}
                          onClick={() => updateActiveSession((s) => ({ ...s, imageQuality: "hd" }))}
                        >
                          {t("playground.hd")}
                        </button>
                      </div>
                    </div>
                  </>
                )}
              </div>
            )}

            {/* Messages area */}
            <div className="playground-messages">
              {activeSession.messages.length === 0 && (
                <div className="playground-empty">
                  <div className="playground-empty-icon">
                    {activeSession.mode === "chat" ? (
                      <svg
                        width="32"
                        height="32"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        strokeWidth="1.2"
                        strokeLinecap="round"
                        strokeLinejoin="round"
                      >
                        <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
                      </svg>
                    ) : (
                      <svg
                        width="32"
                        height="32"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        strokeWidth="1.2"
                        strokeLinecap="round"
                        strokeLinejoin="round"
                      >
                        <rect x="3" y="3" width="18" height="18" rx="2" ry="2" />
                        <circle cx="8.5" cy="8.5" r="1.5" />
                        <path d="M21 15l-5-5L5 21" />
                      </svg>
                    )}
                  </div>
                  <div>{t("playground.emptyHint")}</div>
                  <div className="hint">
                    {activeSession.mode === "chat"
                      ? t("playground.chatHint")
                      : t("playground.imageHint")}
                  </div>
                </div>
              )}

              {activeSession.messages.map((msg) => (
                <div
                  key={msg.id}
                  className={`playground-msg playground-msg-${msg.role}`}
                >
                  <div className="playground-msg-avatar">
                    {msg.role === "user" ? (
                      <svg
                        width="14"
                        height="14"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        strokeWidth="1.8"
                      >
                        <path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2" />
                        <circle cx="12" cy="7" r="4" />
                      </svg>
                    ) : (
                      <svg
                        width="14"
                        height="14"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        strokeWidth="1.8"
                      >
                        <path d="M12 2L2 7l10 5 10-5-10-5z" />
                        <path d="M2 17l10 5 10-5" />
                        <path d="M2 12l10 5 10-5" />
                      </svg>
                    )}
                  </div>
                  <div className="playground-msg-body">
                    <div className="playground-msg-meta">
                      <span className="playground-msg-role">
                        {msg.role === "user" ? t("playground.you") : "AI"}
                      </span>
                      {msg.model && (
                        <span className="playground-msg-model">{msg.model}</span>
                      )}
                      <span className="playground-msg-time">
                        {formatTime(msg.timestamp)}
                      </span>
                      {msg.tokens && (
                        <span className="playground-msg-tokens">
                          {t("keys.tokensShort", { count: msg.tokens.input + msg.tokens.output })}
                        </span>
                      )}
                    </div>
                    <div className="playground-msg-content">
                      {msg.imageUrls && msg.imageUrls.length > 0 && (
                        <div className="playground-ref-grid">
                          {msg.imageUrls.map((url, idx) => (
                            <button
                              key={`${msg.id}-ref-${idx}`}
                              className="playground-ref-thumb"
                              onClick={() => setPreviewImageUrl(url)}
                              title={t("playground.viewLarge")}
                            >
                              <img src={url} alt={`Reference ${idx + 1}`} />
                            </button>
                          ))}
                        </div>
                      )}
                      {msg.imageUrl && (
                        <div className="playground-img-wrap">
                          <img
                            src={msg.imageUrl}
                            alt="Generated"
                            className="playground-img"
                            loading="lazy"
                            onClick={() => setPreviewImageUrl(msg.imageUrl!)}
                            style={{ cursor: "zoom-in" }}
                          />
                          <div className="playground-img-actions">
                            <button
                              className="playground-img-btn"
                              onClick={() => setPreviewImageUrl(msg.imageUrl!)}
                              title={t("playground.viewLarge")}
                            >
                              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                                <circle cx="11" cy="11" r="8" />
                                <line x1="21" y1="21" x2="16.65" y2="16.65" />
                                <line x1="11" y1="8" x2="11" y2="14" />
                                <line x1="8" y1="11" x2="14" y2="11" />
                              </svg>
                              {t("playground.viewLarge")}
                            </button>
                            <button
                              className="playground-img-btn"
                              disabled={savingImages[msg.imageUrl]}
                              onClick={() => saveImage(msg.imageUrl!)}
                              title={t("playground.saveImage")}
                            >
                              {savingImages[msg.imageUrl] ? (
                                <>
                                  <span className="spinner" style={{ width: 10, height: 10 }} />
                                  {t("playground.saving")}
                                </>
                              ) : (
                                <>
                                  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                                    <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
                                    <polyline points="7 10 12 15 17 10" />
                                    <line x1="12" y1="15" x2="12" y2="3" />
                                  </svg>
                                  {t("playground.saveImage")}
                                </>
                              )}
                            </button>
                          </div>
                        </div>
                      )}
                      <div className="playground-msg-text">{msg.content}</div>
                    </div>
                  </div>
                </div>
              ))}

              {sending && (
                <div className="playground-msg playground-msg-assistant">
                  <div className="playground-msg-avatar">
                    <svg
                      width="14"
                      height="14"
                      viewBox="0 0 24 24"
                      fill="none"
                      stroke="currentColor"
                      strokeWidth="1.8"
                    >
                      <path d="M12 2L2 7l10 5 10-5-10-5z" />
                      <path d="M2 17l10 5 10-5" />
                      <path d="M2 12l10 5 10-5" />
                    </svg>
                  </div>
                  <div className="playground-msg-body">
                    <div className="playground-typing">
                      <span className="playground-dot" />
                      <span className="playground-dot" />
                      <span className="playground-dot" />
                    </div>
                  </div>
                </div>
              )}

              <div ref={messagesEndRef} />
            </div>

            {/* Error */}
            {error && (
              <div className="alert err toast-in" style={{ margin: "0 0 8px" }}>
                <div style={{ flex: 1 }}>
                  <strong>{t("tools.failed")}</strong>
                  <pre>{error}</pre>
                </div>
              </div>
            )}

            {/* Input area */}
            <div className="playground-input-area">
              {activeSession.mode === "image" && uploadedImages.length > 0 && (
                <div className="playground-upload-preview">
                  {uploadedImages.map((img) => (
                    <div key={img.id} className="playground-upload-item" title={img.name}>
                      <img src={img.dataUrl} alt={img.name} />
                      <button
                        type="button"
                        className="playground-upload-remove"
                        onClick={() => removeUploadedImage(img.id)}
                        title={t("playground.removeImage")}
                      >
                        ×
                      </button>
                    </div>
                  ))}
                </div>
              )}
              <div className="playground-input-row">
                {activeSession.mode === "image" && (
                  <>
                    <input
                      ref={fileInputRef}
                      type="file"
                      accept="image/*"
                      multiple
                      onChange={onUploadImages}
                      style={{ display: "none" }}
                    />
                    <button
                      type="button"
                      className="btn ghost playground-attach-btn"
                      onClick={() => fileInputRef.current?.click()}
                      disabled={sending || !isModelValid}
                      title={t("playground.uploadImage")}
                    >
                      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                        <rect x="3" y="3" width="18" height="18" rx="2" ry="2" />
                        <circle cx="8.5" cy="8.5" r="1.5" />
                        <path d="M21 15l-5-5L5 21" />
                      </svg>
                    </button>
                  </>
                )}
                <textarea
                  ref={inputRef}
                  className="playground-textarea"
                  value={input}
                  onChange={(e) => setInput(e.target.value)}
                  onKeyDown={onKeyDown}
                  placeholder={
                    !isModelValid
                      ? "无可用模型，请先成功拉取或选择包含可用模型的密钥"
                      : activeSession.mode === "chat"
                        ? t("playground.chatPlaceholder")
                        : t("playground.imagePlaceholder")
                  }
                  rows={1}
                  disabled={sending || !isModelValid}
                />
                <button
                  className="btn primary playground-send-btn"
                  onClick={onSend}
                  disabled={sending || !input.trim() || !isModelValid}
                >
                  {sending ? (
                    <span className="spinner" />
                  ) : (
                    <svg
                      width="16"
                      height="16"
                      viewBox="0 0 24 24"
                      fill="none"
                      stroke="currentColor"
                      strokeWidth="2"
                      strokeLinecap="round"
                      strokeLinejoin="round"
                    >
                      <line x1="22" y1="2" x2="11" y2="13" />
                      <polygon points="22 2 15 22 11 13 2 9 22 2" />
                    </svg>
                  )}
                </button>
              </div>
              <div className="playground-input-hints">
                <span>
                  {t("playground.usingKey")}: <strong>{activeKeyLabel}</strong>
                </span>
                <span>·</span>
                <span>
                  {t("playground.model")}: <strong>{isModelValid ? activeSession.selectedModel : "—"}</strong>
                </span>
                <span>·</span>
                {activeSession.mode === "image" && (
                  <>
                    <span>
                      {uploadedImages.length > 0
                        ? t("playground.imageToImageOn", { count: uploadedImages.length })
                        : t("playground.imageToImageOff")}
                    </span>
                    <span>·</span>
                  </>
                )}
                <span className="hint">{t("playground.enterToSend")}</span>
              </div>
            </div>
          </>
        )}
      </div>

      {previewImageUrl && (
        <div className="playground-preview-overlay" onClick={() => setPreviewImageUrl(null)}>
          <div className="playground-preview-content" onClick={(e) => e.stopPropagation()}>
            <img src={previewImageUrl} alt="Preview" className="playground-preview-img" />
            <button className="playground-preview-close" onClick={() => setPreviewImageUrl(null)}>
              ✕
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
