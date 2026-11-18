// Catbot's extension for Pi (written by Catbot; changes here are overwritten).
//
// It stops each step that would change something and asks Catbot, which shows the person a card,
// and it gives Pi Catbot's own tools. The bot's approval level comes in CATBOT_PI_MODE:
// "ask" asks for edits and commands, "edits" asks for commands, "full" asks nothing, "plan"
// refuses changes. Catbot's MCP server comes in CATBOT_PI_MCP as JSON.

export default function (pi: any) {
	const mode = process.env.CATBOT_PI_MODE ?? "ask";
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
		if (mode === "edits" && edits.has(name)) return;
		const ok = await ctx.ui.confirm("catbot-approval", JSON.stringify({ tool: name, input: event.input }));
		if (!ok) return { block: true, reason: "The person said no." };
	});
}
