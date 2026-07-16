declare module 'adi:workforce/host' {
    function callTool(name: string, argsJson: string): string;
    function callLlm(system: string, user: string): string;
    function log(level: string, message: string): void;
    function getContext(key: string): string | null;
    function readFile(path: string): string;
    function writeFile(path: string, content: string): void;
    function loopInit(configJson: string): string;
    function loopLlm(sessionId: string, turnsJson: string): string;
    function loopTool(sessionId: string, toolName: string, argsJson: string): string;
    function loopFinish(sessionId: string): void;
    function subscribe(name: string, configJson: string): void;
}
