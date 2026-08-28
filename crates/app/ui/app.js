const doctorButton = document.querySelector("#doctor");
const result = document.querySelector("#result");

doctorButton.addEventListener("click", async () => {
  doctorButton.disabled = true;
  doctorButton.textContent = "Running…";
  result.textContent = "Checking local runtime…";
  try {
    result.textContent = await window.__TAURI__.core.invoke("doctor");
  } catch (error) {
    result.textContent = `Doctor failed\n${String(error)}`;
  } finally {
    doctorButton.disabled = false;
    doctorButton.textContent = "Run doctor";
  }
});
