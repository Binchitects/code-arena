import { render } from "solid-js/web";
import { App } from "./app";
import { connect } from "./stream";
import "./style.css";

connect();
render(() => <App />, document.getElementById("root")!);
