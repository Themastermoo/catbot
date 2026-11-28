// Catbot's extension for Pi (written by Catbot; changes here are overwritten).
//
// It stops each step that would change something and asks Catbot, which shows the person a card,
// and it gives Pi Catbot's own tools. The bot's approval level comes in CATBOT_PI_MODE:
// "ask" asks for edits and commands, "edits" asks for commands (and edits outside workspace roots),
// "full" asks nothing, "plan" refuses changes. Catbot's MCP server comes in CATBOT_PI_MCP as JSON.

import * as fs from "node:fs";
import * as path from "node:path";

function resolveReal(target: string): string {
	let cur = path.resolve(process.cwd(), target);
	const tail: string[] = [];
	while (true) {
		try {
			const real = fs.realpathSync(cur);
			return tail.length ? path.join(real, ...tail.reverse()) : real;
		} catch {
			const parent = path.dirname(cur);
			if (parent === cur) return cur;
			tail.push(path.basename(cur));
			cur = parent;
		}
	}
}

function insideRoots(target: unknown, roots: string[]): boolean {
	if (typeof target !== "string" || !target.trim() || roots.length === 0) return false;
	const real = resolveReal(target.trim());
	return roots.some((root) => real === root || real.startsWith(root + path.sep));
}

export default function (pi: any) {
	const mode = process.env.CATBOT_PI_MODE ?? "ask";
	let roots: string[] = [];
	try {
		const raw = JSON.parse(process.env.CATBOT_PI_ROOTS ?? "[]");
		if (Array.isArray(raw)) roots = raw.filter((r): r is string => typeof r === "string" && r.length > 0);
	} catch {}
	const server = process.env.CATBOT_PI_MCP;
	if (server) {
		try {
			pi.registerMcpServer("catbot", { ...JSON.parse(server), exposure: "direct" });
		} catch (e) {
			console.error(`catbot: couldn't add Catbot's tools: ${e}`);
		}
	}
	const free = new Set(["read", "grep", "find", "ls", "codemode", "tool_search"]);
	const edits = new Set(["edit", "write"]);
	pi.on("tool_call", async (event: any, ctx: any) => {
		const name: string = event.toolName;
		if (free.has(name) || name.startsWith("mcp__catbot__")) return;
		if (mode === "full") return;
		if (mode === "plan") return { block: true, reason: "Catbot's plan mode: nothing is changed, only planned." };
		if (mode === "edits" && edits.has(name) && insideRoots(event.input?.path ?? event.input?.file_path, roots)) return;
		const ok = await ctx.ui.confirm("catbot-approval", JSON.stringify({ tool: name, input: event.input }));
		if (!ok) return { block: true, reason: "The person said no." };
	});
}
