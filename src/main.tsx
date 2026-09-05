import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";

// 屏蔽原生右键菜单：应用级 UI，右键不应弹出浏览器/WebView 的默认菜单。
window.addEventListener("contextmenu", (e) => e.preventDefault());

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
