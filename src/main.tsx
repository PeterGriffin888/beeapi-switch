import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles.css";

// Disable right-click context menu and F12/DevTools shortcuts in production.
document.addEventListener("contextmenu", (e) => e.preventDefault());
document.addEventListener("keydown", (e) => {
  if (
    e.key === "F12" ||
    (e.ctrlKey && e.shiftKey && (e.key === "I" || e.key === "i")) ||
    (e.ctrlKey && e.shiftKey && (e.key === "J" || e.key === "j")) ||
    (e.ctrlKey && (e.key === "U" || e.key === "u"))
  ) {
    e.preventDefault();
  }
});

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
