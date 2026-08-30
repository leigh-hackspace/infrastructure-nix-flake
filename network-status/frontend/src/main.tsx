import "./styles.css";
import { render } from "solid-js/web";
import { App } from "./app";

const root = document.getElementById("app");
if (root) {
  render(() => <App />, root);
}
