// Запуск веб-клиента Rustspace: модуль WebAssembly рисует на <canvas>.
import init from "./rustspace.js";

const shell = window.backspace;
if (shell) {
  // Десктоп Backspace (Electron): окно без рамки и проверка, что страница
  // ожила, — без сигнала через 20 секунд он показывает экран восстановления.
  document.body.classList.add("desktop");
  if (shell.platform === "darwin") document.body.classList.add("darwin");
  if (typeof shell.rendererReady === "function") shell.rendererReady();
}

const english = !(navigator.language || "").toLowerCase().startsWith("ru");

init()
  .then(() => document.body.classList.add("ready"))
  .catch((error) => {
    const note = document.querySelector("#loading p");
    if (note) {
      note.textContent = english
        ? "Rustspace did not start in this browser: " + error
        : "Rustspace не запустился в этом браузере: " + error;
    }
    console.error(error);
  });
