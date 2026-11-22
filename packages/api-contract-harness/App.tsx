import { host, mount, signal, Text, View } from "nsh";
import {
  backToLauncher,
  calendarHolidays,
  cancelProcess,
  cancelWalk,
  clipboardHistory,
  closePage,
  closeWindow,
  confirm,
  copyText,
  deleteSecret,
  fetchHttp,
  filesystemGrants,
  getSecret,
  HOST_CALL_FIXTURES,
  installedApplications,
  listSecrets,
  openPath,
  openWith,
  pickFolder,
  readText,
  reportProgress,
  revealPath,
  runProcess,
  setSecret,
  setPageTitle,
  StewardError,
  storageGet,
  storageSet,
  toast,
  walk,
  writeText,
} from "@steward/extension-api";

function fixture(name: string) {
  const value = HOST_CALL_FIXTURES.find((candidate) => candidate.name === name);
  if (!value) throw new Error(`Missing contract fixture ${name}`);
  return value;
}

async function invoke(name: string): Promise<string> {
  const value = fixture(name);
  const request = value.request as never;
  const processId = "contract-process-1";
  switch (name) {
    case "steward.preferences.get":
    case "steward.preferences.all":
      await host.call(name, request);
      return "";
    case "steward.feedback.toast":
      await toast(request);
      return "";
    case "steward.feedback.confirm":
      await confirm(request);
      return "";
    case "steward.feedback.progress":
      await reportProgress(request);
      return "";
    case "steward.clipboard.history":
      await clipboardHistory();
      return "";
    case "steward.clipboard.write":
      await copyText((request as { text: string }).text);
      return "";
    case "steward.storage.get":
      await storageGet((request as { key: string }).key);
      return "";
    case "steward.storage.set":
      await storageSet((request as { key: string }).key, (request as { value: string }).value);
      return "";
    case "steward.secrets.get":
      await getSecret((request as { key: string }).key);
      return "";
    case "steward.secrets.set": {
      const input = request as { key: string; value: string };
      await setSecret(input.key, input.value);
      return "";
    }
    case "steward.secrets.delete":
      await deleteSecret((request as { key: string }).key);
      return "";
    case "steward.secrets.list":
      await listSecrets();
      return "";
    case "steward.calendar.holidays":
      await calendarHolidays((request as { year: number }).year);
      return "";
    case "steward.window.close":
      closeWindow();
      return "";
    case "steward.navigation.back":
      await backToLauncher();
      return "";
    case "steward.navigation.close":
      await closePage();
      return "";
    case "steward.navigation.setTitle":
      await setPageTitle((request as { title: string }).title);
      return "";
    case "steward.fs.pick": {
      const mode = (request as { mode: string }).mode;
      await pickFolder(mode === "folderWrite");
      return "";
    }
    case "steward.fs.cancelWalk":
      await cancelWalk((request as { id: string }).id);
      return "";
    case "steward.fs.walk":
      await walk(request);
      return "";
    case "steward.fs.readText": {
      const input = request as { path: string; maxBytes: number };
      await readText(input.path, input.maxBytes);
      return "";
    }
    case "steward.fs.writeText": {
      const input = request as { path: string; contents: string };
      await writeText(input.path, input.contents);
      return "";
    }
    case "steward.fs.grants":
      await filesystemGrants();
      return "";
    case "steward.system.applications":
      await installedApplications();
      return "";
    case "steward.system.openPath":
      await openPath((request as { path: string }).path);
      return "";
    case "steward.system.revealPath":
      await revealPath((request as { path: string }).path);
      return "";
    case "steward.system.openWith": {
      const input = request as { applicationId: string; path: string };
      await openWith(input.applicationId, input.path);
      return "";
    }
    case "steward.net.fetch":
      await fetchHttp(request);
      return "";
    case "steward.exec.run": {
      const input = request as {
        executable: string;
        arguments: string[];
        workingDirectory: string;
        timeoutMs: number;
        maxOutputBytes: number;
      };
      const result = await runProcess({ id: processId, ...input });
      return `process:${result.requestId}`;
    }
    case "steward.exec.cancel":
      await cancelProcess(processId);
      return "";
    default:
      throw new Error(`No SDK invocation for contract call ${name}`);
  }
}

function App() {
  const status = signal("ready");

  async function invokeOne(name: string) {
    try {
      const result = await invoke(name);
      status.set(result || "ok");
    } catch (error) {
      status.set(error instanceof Error ? error.message : String(error));
    }
  }

  async function invokeStructuredError() {
    try {
      await copyText("contract error");
      status.set("missing error");
    } catch (error) {
      if (error instanceof StewardError) {
        status.set(`${error.code}:${error.retryable}:${error.message}`);
      } else {
        status.set(`wrong error:${String(error)}`);
      }
    }
  }

  async function invokeQuotaError() {
    try {
      await copyText("x".repeat(100_001));
      status.set("missing quota error");
    } catch (error) {
      if (error instanceof StewardError) {
        status.set(`${error.code}:${error.retryable}:${error.message}`);
      } else {
        status.set(`wrong error:${String(error)}`);
      }
    }
  }

  async function invokeSecretQuotaError() {
    try {
      await setSecret("github-token", "x".repeat(2561));
      status.set("missing secret quota error");
    } catch (error) {
      if (error instanceof StewardError) {
        status.set(`${error.code}:${error.retryable}:${error.message}`);
      } else {
        status.set(`wrong error:${String(error)}`);
      }
    }
  }

  return (
    <View style={{ flex: 1, padding: 10, gap: 4 }}>
      <Text>Status: {status()}</Text>
      <View role="button" onPress={invokeStructuredError}>
        <Text>structured error</Text>
      </View>
      <View role="button" onPress={invokeQuotaError}>
        <Text>quota error</Text>
      </View>
      <View role="button" onPress={invokeSecretQuotaError}>
        <Text>secret quota error</Text>
      </View>
      <Each of={HOST_CALL_FIXTURES} key={(call) => call.name}>
        {(call) => (
          <View role="button" onPress={() => invokeOne(call().name)}>
            <Text>{call().name}</Text>
          </View>
        )}
      </Each>
    </View>
  );
}

mount(App);
