import json
import os
import re
import shutil
import subprocess
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
ENTRY = ROOT / "scripts" / "runtime_acceptance_0_7.ps1"
STAGE = ROOT / "scripts" / "runtime_acceptance_0_7_message.ps1"
SUPPORT = ROOT / "scripts" / "runtime_acceptance_0_7_support.ps1"
CLIENT = ROOT / "scripts" / "message_runtime_acceptance_client.mjs"


class MessageRuntimeAcceptancePolicyTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.entry = ENTRY.read_text(encoding="utf-8")
        cls.stage = STAGE.read_text(encoding="utf-8")
        cls.support = SUPPORT.read_text(encoding="utf-8")
        cls.client = CLIENT.read_text(encoding="utf-8")

    def test_stage_uses_the_locked_contract_and_exact_opt_in(self) -> None:
        parameter_end = self.stage.index(")\n\nSet-StrictMode")
        parameter_block = self.stage[self.stage.index("param(") : parameter_end]
        for parameter in (
            "$ConfirmRun",
            "$ProjectName",
            "$RunDirectory",
            "$DockerExecutable",
            "$DockerContext",
            "$OwnershipToken",
            "$DockerHelperPath",
        ):
            self.assertIn(parameter, parameter_block)
        self.assertNotIn("[switch]", parameter_block)
        guard = 'if ($ConfirmRun -cne "RUN-RYFRAME-V0-7-STAGE")'
        self.assertEqual(self.stage.count(guard), 1)
        guard_position = self.stage.index(guard)
        for side_effect in (
            "Start-Transcript",
            '"compose", "--project-name"',
            "Start-MessageAcceptanceProcess",
        ):
            self.assertLess(guard_position, self.stage.index(side_effect))
        self.assertIn("runtime_acceptance_0_7_message.ps1", self.entry)

    def test_stage_is_scoped_to_the_parent_project_and_evidence_directory(self) -> None:
        for fragment in (
            "Assert-RyFrameV07ProjectName -ProjectName $ProjectName",
            'Join-Path $targetDirectory "runtime-acceptance-0-7"',
            "$resolvedRunDirectory.StartsWith($targetPrefix, $runPathComparison)",
            'Join-Path $resolvedRunDirectory "message-run.json"',
            "Test-MessageAcceptanceSamePath -Actual $DockerHelperPath -Expected $expectedHelperPath",
            '"compose", "--project-name", $ProjectName,',
            '"--file", $composeFile,',
        ):
            self.assertIn(fragment, self.stage)
        self.assertNotIn("Remove-Item", self.stage)
        self.assertNotIn("runtime_acceptance.ps1", self.stage)

    def test_process_lifecycle_uses_owned_captured_process_objects(self) -> None:
        for fragment in (
            "Get-RyFrameV07OwnedProcess `",
            "Stop-Process -InputObject $ownedProcess -ErrorAction Stop",
            "$ownedProcess.WaitForExit(10000)",
            "Stop-Process -InputObject $ownedProcess -Force -ErrorAction Stop",
            "$ownedProcess.WaitForExit($TimeoutSeconds * 1000)",
            "ConvertTo-RyFrameV07ProcessArgument -Value $_",
            ') -join " ")',
        ):
            self.assertIn(fragment, self.stage)
        for forbidden in (
            "Get-Process -Id",
            "Stop-Process -Id",
            "Wait-Process -Id",
            "$startArguments.ArgumentList = $Arguments",
        ):
            self.assertNotIn(forbidden, self.stage)

        assert_start = self.stage.index("function Assert-MessageAcceptanceProcess")
        stop_start = self.stage.index("function Stop-MessageAcceptanceProcess", assert_start)
        readiness_start = self.stage.index("function Wait-MessageAcceptanceReadiness", stop_start)
        assert_block = self.stage[assert_start:stop_start]
        stop_block = self.stage[stop_start:readiness_start]
        self.assertIn("return $ownedProcess", assert_block)
        graceful = stop_block.index("Stop-Process -InputObject $ownedProcess -ErrorAction Stop")
        graceful_wait = stop_block.index("$ownedProcess.WaitForExit(10000)", graceful)
        revalidate = stop_block.index("$ownedProcess = Assert-MessageAcceptanceProcess", graceful_wait)
        forced = stop_block.index(
            "Stop-Process -InputObject $ownedProcess -Force -ErrorAction Stop",
            revalidate,
        )
        self.assertLess(graceful, graceful_wait)
        self.assertLess(graceful_wait, revalidate)
        self.assertLess(revalidate, forced)

        wait_start = self.stage.index("function Wait-MessageAcceptanceProcessExit")
        wait_end = self.stage.index("function Write-MessageAcceptanceSignal", wait_start)
        wait_block = self.stage[wait_start:wait_end]
        self.assertIn("Get-RyFrameV07OwnedProcess", wait_block)
        self.assertIn("WaitForExit", wait_block)
        self.assertNotIn("while (", wait_block)

    def test_environment_is_fully_snapshotted_and_restored_before_final_metadata(self) -> None:
        for fragment in (
            "function Get-RyFrameV07ProcessEnvironmentSnapshot",
            "function Restore-RyFrameV07ProcessEnvironmentSnapshot",
            '[System.Environment]::GetEnvironmentVariables("Process")',
            "$environmentSnapshot = Get-RyFrameV07ProcessEnvironmentSnapshot",
            "Restore-RyFrameV07ProcessEnvironmentSnapshot -Snapshot $environmentSnapshot",
            "$script:MessageAcceptanceMessages.EnvironmentRestore -f $_.Exception.Message",
        ):
            self.assertIn(fragment, self.stage + self.support)
        snapshot = self.stage.index(
            "$environmentSnapshot = Get-RyFrameV07ProcessEnvironmentSnapshot"
        )
        stage_try = self.stage.index("try {", snapshot)
        finally_start = self.stage.rindex("finally {")
        restore = self.stage.index(
            "Restore-RyFrameV07ProcessEnvironmentSnapshot -Snapshot $environmentSnapshot",
            finally_start,
        )
        completed = self.stage.index('$metadata["completed_at"]', restore)
        final_write = self.stage.index(
            "Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath",
            completed,
        )
        self.assertLess(snapshot, stage_try)
        self.assertLess(finally_start, restore)
        self.assertLess(restore, completed)
        self.assertLess(completed, final_write)

    def test_context_mismatch_and_image_evidence_are_explicit(self) -> None:
        for fragment in (
            '"ContextMismatch":',
            "$script:MessageAcceptanceMessages.ContextMismatch -f $contextInfo.Name, $DockerContext",
            "images = @()",
            "$imageEvidence = @(Get-RyFrameV07ProjectImageEvidence `",
            '$metadata["images"] = $imageEvidence',
            '($imageServices -join ",") -cne "mysql,redis,rustfs"',
            '"up", "-d", "--wait", "mysql", "redis", "rustfs"',
            "configured_image = [string]$container.Config.Image",
            "image_id = $imageId",
            "repo_digests = @($imageDocuments[0].RepoDigests | Sort-Object)",
        ):
            self.assertIn(fragment, self.stage + self.support)
        compose_up = self.stage.index('"up", "-d", "--wait"')
        images = self.stage.index("Get-RyFrameV07ProjectImageEvidence", compose_up)
        evidence_write = self.stage.index(
            "Write-RyFrameV07MetadataAtomically -Metadata $metadata -Path $metadataPath",
            images,
        )
        self.assertLess(compose_up, images)
        self.assertLess(images, evidence_write)

    def test_docker_project_requires_an_exact_ownership_token_and_empty_project(self) -> None:
        for fragment in (
            "[string]$OwnershipToken",
            "Assert-RyFrameV07OwnershipToken -OwnershipToken $OwnershipToken",
            "ownership_token = $OwnershipToken",
            'Join-Path $repositoryRoot "deploy/tests/runtime-acceptance-0-7-ownership.compose.yml"',
            'Set-MessageAcceptanceEnvironment -Name "RYFRAME_V07_OWNERSHIP_TOKEN" -Value $OwnershipToken',
            '"--file", $ownershipComposeFile',
            "Assert-RyFrameV07ProjectEmpty `",
            "Get-RyFrameV07ProjectImageEvidence `",
            "Stop-RyFrameV07DockerService `",
            "Restore-RyFrameV07DockerFault `",
            "Remove-RyFrameV07DockerProjectResources `",
        ):
            self.assertIn(fragment, self.stage)
        self.assertIn('^ryframe-v07-owner-[a-f0-9]{32}$', self.support)

        config = self.stage.index('$script:MessageAcceptanceMessages.ComposeValidate')
        empty = self.stage.index("Assert-RyFrameV07ProjectEmpty `", config)
        owned = self.stage.index("$dockerOwned = $true", empty)
        compose_up = self.stage.index('$script:MessageAcceptanceMessages.ComposeStart', owned)
        self.assertLess(config, empty)
        self.assertLess(empty, owned)
        self.assertLess(owned, compose_up)

        for command in (
            "Get-RyFrameV07ProjectImageEvidence `",
            "Stop-RyFrameV07DockerService `",
            "Restore-RyFrameV07DockerFault `",
            "Remove-RyFrameV07DockerProjectResources `",
        ):
            search_from = 0
            while True:
                start = self.stage.find(command, search_from)
                if start < 0:
                    break
                next_command = self.stage.find("\n    ", start + len(command))
                block_end = min(
                    len(self.stage),
                    start + 500 if next_command < 0 else max(start + 500, next_command),
                )
                self.assertIn("-OwnershipToken $OwnershipToken", self.stage[start:block_end])
                search_from = start + len(command)

    def test_client_preflight_guards_run_before_network_and_prevent_overwrite(self) -> None:
        for fragment in (
            'const INTERNAL_ACCEPTANCE_TOKEN = "RUN-RYFRAME-V0-7-MESSAGE-CLIENT"',
            '"--internal-token"',
            'parsed.get("--internal-token") !== INTERNAL_ACCEPTANCE_TOKEN',
            "rawValue !== url.origin",
            'url.protocol !== "http:"',
            'url.hostname !== "127.0.0.1"',
            'path.join(REPOSITORY_ROOT, "target", "runtime-acceptance-0-7")',
            "assertPathWithinAcceptanceTarget(controlDirectory",
            "const resolved = path.resolve(controlDirectory, filename)",
            "realpath(ACCEPTANCE_TARGET_ROOT)",
            "await assertPreflightFilesystem(controlDirectory, [",
            'const handle = await open(temporaryPath, "wx")',
            "await link(temporaryPath, filePath)",
            'error?.code === "EEXIST"',
            '"--internal-token", "RUN-RYFRAME-V0-7-MESSAGE-CLIENT"',
        ):
            self.assertIn(fragment, self.stage + self.client)

        preflight = self.client.index("await assertPreflightFilesystem(controlDirectory, [")
        login = self.client.index("const { token, userId } = await login(apiBase)")
        self.assertLess(preflight, login)
        self.assertNotIn("rename(", self.client)

        for filename in (
            "client-ready.json",
            "tenant-fixture.json",
            "tenant-result.json",
            "redis-fault-fixture.json",
            "client-delivered.json",
            "redis-restored.signal",
            "cleanup-ready.json",
            "cleanup-result.json",
            "client-result.json",
        ):
            self.assertIn(f'controlPath(controlDirectory, "{filename}")', self.client)

    def test_two_api_instances_share_real_dependencies(self) -> None:
        for name in ("mysql", "redis", "rustfs", "api_a", "api_b"):
            self.assertIn(f'"{name}"', self.stage)
        for fragment in (
            'Set-MessageAcceptanceEnvironment -Name "APP_REDIS_MODE" -Value "optional"',
            'Set-MessageAcceptanceEnvironment -Name "APP_JOBS_MODE" -Value "external"',
            'Set-MessageAcceptanceEnvironment -Name "APP_RATE_LIMIT_ENABLED" -Value "false"',
            'Set-MessageAcceptanceEnvironment -Name "APP_MESSAGING_ENABLED" -Value "true"',
            'Set-MessageAcceptanceEnvironment -Name "APP_MESSAGING_RETENTION_DAYS" -Value "90"',
            'Set-MessageAcceptanceEnvironment -Name "TOKIO_WORKER_THREADS" -Value "1"',
            'Set-MessageAcceptanceEnvironment -Name "APP_MESSAGING_REPLAY_INTERVAL_SECONDS" -Value "3"',
            'Set-MessageAcceptanceEnvironment -Name "APP_MESSAGING_REPLAY_JITTER_SECONDS" -Value "0"',
            'Set-MessageAcceptanceEnvironment -Name "SNOWFLAKE_WORKER_ID" -Value "901"',
            'Set-MessageAcceptanceEnvironment -Name "SNOWFLAKE_WORKER_ID" -Value "902"',
            '"http://127.0.0.1:$($ports.api_a)/readyz"',
            '"http://127.0.0.1:$($ports.api_b)/readyz"',
        ):
            self.assertIn(fragment, self.stage)
        self.assertEqual(self.stage.count('Start-MessageAcceptanceProcess `\n        -Executable $apiBinary'), 2)
        self.assertNotIn('"test", "--locked"', self.stage)

    def test_redis_interruption_is_native_observed_and_recovered(self) -> None:
        stop = self.stage.index("Stop-RyFrameV07DockerService")
        fixture_sql = self.stage.index("$redisFaultFixtureSql", stop)
        fixture_signal = self.stage.index("-Path $redisFaultFixturePath", fixture_sql)
        delivered = self.stage.index("-Path $clientDeliveredPath", fixture_signal)
        restore = self.stage.index("Restore-RyFrameV07DockerFault", delivered)
        restored_signal = self.stage.index(
            "Write-MessageAcceptanceSignal -Path $redisRestoredSignal", restore
        )
        self.assertLess(stop, fixture_sql)
        self.assertLess(fixture_sql, fixture_signal)
        self.assertLess(fixture_signal, delivered)
        self.assertLess(delivered, restore)
        self.assertLess(restore, restored_signal)
        for fragment in (
            'listener_metric = "ryframe_message_redis_listener_connected"',
            '-MetricName "ryframe_message_redis_listener_connected"',
            "-ExpectedValue 0",
            "-ExpectedValue 1",
            '$metadata["redis_fault"]["interrupted_instance_count"] = 2',
            '$metadata["redis_fault"]["restored_instance_count"] = 2',
            "runtime_acceptance_0_7_redis_fault",
            'Assert-MessageAcceptanceSqlResult -Lines $redisFaultFixtureLines -Expected "1:1:1"',
        ):
            self.assertIn(fragment, self.stage)
        self.assertIn('fixture_source: "mysql"', self.client)
        self.assertEqual(self.stage.count("Wait-MessageAcceptanceMetric `"), 6)
        self.assertNotIn("Wait-MessageAcceptanceLogCount", self.stage)
        self.assertNotIn("Get-MessageAcceptanceLogCount", self.stage)
        self.assertNotIn("toxiproxy", (self.stage + self.client).lower())
        self.assertNotRegex(self.stage, r'@\("(?:container|network)",\s*"(?:stop|start|disconnect|connect)"')

    def test_client_proves_multi_connection_bilingual_logical_delivery(self) -> None:
        for fragment in (
            'const expectedZh = "欢迎 redis-fault-proof"',
            'const expectedEn = "Welcome redis-fault-proof"',
            '{ locale: "zh-CN", text: expectedZh, label: "中文连接一" }',
            '{ locale: "en-US", text: expectedEn, label: "英文连接" }',
            '{ locale: "zh-CN", text: expectedZh, label: "中文连接二" }',
            'label: "API-B 英文连接"',
            'headers: authHeaders(token, locale)',
            "targetMessageIds: new Set()",
            "targetRawFrameCount: 0",
            "armProbeTarget(",
            "allProbes.every((probe) => probe.state.targetMessageIds.size === 1)",
            'probes[0].socket.send(JSON.stringify({ v: 1, type: "ack", ids: [messageId] }))',
            "probes[0].state.acknowledgedIds.has(messageId)",
            "logical_message_count: probe.state.targetMessageIds.size",
            "raw_frame_count: probe.state.targetRawFrameCount",
            'assertInboxRendering(apiBase, token, messageId, "zh-CN", expectedZh)',
            'assertInboxRendering(apiBase, token, messageId, "en-US", expectedEn)',
        ):
            self.assertIn(fragment, self.client)
        for fragment in (
            "'user.welcome', 'user.welcome', JSON_OBJECT('name', 'redis-fault-proof')",
            "INSERT INTO sys_message_audience",
            "INSERT INTO sys_message_recipient",
        ):
            self.assertIn(fragment, self.stage)
        down_wait = self.client.index('"Redis 故障补拉夹具"')
        fixture_read = self.client.index("readJson(redisFaultFixturePath)", down_wait)
        delivered_write = self.client.index("writeJsonAtomically(deliveredPath", fixture_read)
        restore_wait = self.client.index('"Redis 恢复信号"', delivered_write)
        inbox_render = self.client.index("await assertInboxRendering", restore_wait)
        self.assertLess(down_wait, fixture_read)
        self.assertLess(fixture_read, delivered_write)
        self.assertLess(delivered_write, restore_wait)
        self.assertLess(restore_wait, inbox_render)
        fault_window = self.client[fixture_read:delivered_write]
        self.assertNotIn('requestJson(apiBase, "/api/v1/system/messages"', fault_window)

    def test_same_identity_is_active_on_both_instances_with_exact_metrics(self) -> None:
        for fragment in (
            "const secondaryGrant = await issueTicket(",
            "secondaryApiBase,",
            "secondaryProbe = createSocketProbe(",
            "const allProbes = [...probes, secondaryProbe]",
            "await waitForHealthyProbes(allProbes)",
            "alignReplayBaseline(apiBase, probes.length, probes)",
            "alignReplayBaseline(secondaryApiBase, 1, [secondaryProbe])",
            "primaryReplayQueryDelta < 1 || secondaryReplayQueryDelta < 1",
            "primaryDeliveryDelta !== primaryRawFrameCount",
            "secondaryDeliveryDelta !== secondaryRawFrameCount",
            "primary_connection_count: probes.length",
            "secondary_connection_count: 1",
            "total_connection_count: allProbes.length",
            "api_a: {",
            "api_b: {",
            "$clientDelivered.primary_connection_count -ne 3",
            "$clientDelivered.secondary_connection_count -ne 1",
            "$clientDelivered.total_connection_count -ne 4",
            "$clientDelivered.instance_metrics.api_a.replay_query_delta -lt 1",
            "$clientDelivered.instance_metrics.api_a.delivery_delta -ne $clientDelivered.primary_raw_frame_count",
            "$clientDelivered.instance_metrics.api_b.replay_query_delta -lt 1",
            "$clientDelivered.instance_metrics.api_b.delivery_delta -ne $clientDelivered.secondary_raw_frame_count",
        ):
            self.assertIn(fragment, self.stage + self.client)

        primary_grants = self.client.index("const grants = await Promise.all(")
        secondary_grant = self.client.index("const secondaryGrant = await issueTicket(")
        combined = self.client.index("const allProbes = [...probes, secondaryProbe]")
        target_wait = self.client.index(
            "allProbes.every((probe) => probe.state.targetMessageIds.size === 1)"
        )
        ack_send = self.client.index(
            'probes[0].socket.send(JSON.stringify({ v: 1, type: "ack", ids: [messageId] }))',
            target_wait,
        )
        ack_received = self.client.index(
            "probes[0].state.acknowledgedIds.has(messageId)",
            ack_send,
        )
        delivered_write = self.client.index("writeJsonAtomically(deliveredPath", ack_received)
        self.assertLess(primary_grants, secondary_grant)
        self.assertLess(secondary_grant, combined)
        self.assertLess(combined, target_wait)
        self.assertLess(target_wait, ack_send)
        self.assertLess(ack_send, ack_received)
        self.assertLess(ack_received, delivered_write)

    def test_probe_waits_fail_fast_on_terminal_socket_errors(self) -> None:
        for fragment in (
            "async function waitFor(description, timeoutMilliseconds, predicate, terminalError = null)",
            "const fatalError = terminalError?.()",
            "if (fatalError) throw fatalError",
            "function probeTerminalError(probes)",
            "const terminalError = probes.length > 0 ? () => probeTerminalError(probes) : null",
            "() => probeTerminalError([reconnectProbe])",
            "() => probeTerminalError([isolatedProbe])",
        ):
            self.assertIn(fragment, self.client)
        self.assertEqual(self.client.count("() => probeTerminalError(allProbes)"), 7)

    def test_offline_window_acknowledges_logically_once_and_rechecks_new_connection(self) -> None:
        for fragment in (
            "async function assertOfflineReconnect(",
            "settledMetrics(primaryApiBase, 0)",
            "settledMetrics(secondaryApiBase, 0)",
            '"runtime_acceptance_0_7_offline"',
            "whileOffline[index].connections !== 0",
            "whileOffline[index].delivered !== offlineBaselines[index].delivered",
            "const reconnectGrant = await issueTicket(secondaryApiBase",
            "createSocketProbe(\n    secondaryApiBase,",
            "armProbeTarget(",
            "reconnectProbe.state.targetMessageIds.size === 1",
            "reconnectProbe.state.acknowledgedIds.has(messageId)",
            "async function settleSingleProbeDelivery(",
            '"断线重连投递指标与客户端帧稳定"',
            "raw_frame_count: deliveryEvidence.rawFrameCount",
            "async function assertAcknowledgedMessageAbsentAcrossReplayCycles(",
            "post_ack_message_count: postAckMessageCount",
            "verified_across_new_connection: true",
            "primaryAfterReconnect.replaySuccess !== offlineBaselines[0].replaySuccess",
            "await closeProbes([reconnectProbe])",
            "const offlineReconnect = await assertOfflineReconnect(",
            "offline_reconnect: offlineReconnect",
            '$clientReady.offline_reconnect.disconnected_instance -ne "api_a"',
            '$clientReady.offline_reconnect.reconnected_instance -ne "api_b"',
            "$clientReady.offline_reconnect.logical_message_count -ne 1",
            "$clientDelivered.offline_reconnect.raw_frame_count -lt 1",
            "$clientResult.offline_reconnect.ack_persistence.post_ack_message_count -ne 0",
            "delivery_probe_counts: deliveryProbeCounts",
        ):
            self.assertIn(fragment, self.stage + self.client)
        self.assertEqual(
            self.stage.count("offline_reconnect.ack_persistence.alignment_replay_query_delta -lt 1"),
            3,
        )

        offline_call = self.client.index("const offlineReconnect = await assertOfflineReconnect(")
        main_grants = self.client.index("const grants = await Promise.all(", offline_call)
        ready_write = self.client.index("await writeJsonAtomically(readyPath", main_grants)
        self.assertLess(offline_call, main_grants)
        self.assertLess(main_grants, ready_write)

    def test_shared_query_metric_is_stricter_than_connection_count(self) -> None:
        for fragment in (
            'metricValue(text, "ryframe_ws_connections")',
            'metricValue(text, "ryframe_message_replay_query_total", { result: "success" })',
            'metricValue(text, "ryframe_message_delivery_total", { result: "delivered" })',
            "alignReplayBaseline(apiBase, probes.length, probes)",
            "alignReplayBaseline(secondaryApiBase, 1, [secondaryProbe])",
            "primaryReplayQueryDelta < 1 || secondaryReplayQueryDelta < 1",
            "primaryDeliveryDelta !== primaryRawFrameCount",
            "secondaryDeliveryDelta !== secondaryRawFrameCount",
            "replay_query_delta: primaryReplayQueryDelta",
            "delivery_delta: primaryDeliveryDelta",
            "replay_query_delta: secondaryReplayQueryDelta",
            "delivery_delta: secondaryDeliveryDelta",
        ):
            self.assertIn(fragment, self.client)
        for fragment in (
            "$clientDelivered.instance_metrics.api_a.replay_query_delta -lt 1",
            "$clientDelivered.instance_metrics.api_a.delivery_delta -ne $clientDelivered.primary_raw_frame_count",
            "$clientDelivered.instance_metrics.api_b.replay_query_delta -lt 1",
            "$clientDelivered.instance_metrics.api_b.delivery_delta -ne $clientDelivered.secondary_raw_frame_count",
            "$clientResult.instance_metrics.api_a.replay_query_delta -lt 1",
            "$clientResult.instance_metrics.api_b.replay_query_delta -lt 1",
        ):
            self.assertIn(fragment, self.stage)

    def test_deduplication_stability_crosses_a_full_later_replay_cycle(self) -> None:
        for fragment in (
            "function assertTargetProbesStable(probes, label)",
            "function assertProbeCountsUnchanged(probes, expectedCounts, label)",
            "async function waitForReplayAdvance(",
            "async function assertReplayDeduplicationWindow(",
            "current.replaySuccess > baseline.replaySuccess",
            "current.delivered !== baseline.delivered",
            "starting[index],\n      probes,\n      expectedProbeCounts",
            "aligned[index],\n      probes,\n      expectedProbeCounts",
            "completed[index].replaySuccess <= aligned[index].replaySuccess",
            "full_replay_cycle_observed: true",
            "total_replay_query_delta: final.replaySuccess - starting[index].replaySuccess",
            "delivery_delta: final.delivered - starting[index].delivered",
            "async function assertAcknowledgedMessageAbsentAcrossReplayCycles(",
            '"ACK 持久化第一完整补拉周期"',
            '"ACK 持久化第二完整补拉周期"',
            "post_ack_message_count: postAckMessageCount",
            "verified_across_new_connection: true",
            "const deduplicationStability = await assertReplayDeduplicationWindow(",
            '{ name: "api_a", apiBase, connectionCount: probes.length }',
            '{ name: "api_b", apiBase: secondaryApiBase, connectionCount: 1 }',
            "const deliveryEvidence = await settleSingleProbeDelivery(",
            '"关闭前统一检查"',
            "deduplication_stability: {",
            "$clientResult.deduplication_stability.full_replay_cycle_observed -ne $true",
            "$clientResult.deduplication_stability.instance_metrics.api_a.replay_query_delta -lt 1",
            "$clientResult.deduplication_stability.instance_metrics.api_a.total_replay_query_delta -lt 2",
            "$clientResult.deduplication_stability.instance_metrics.api_a.delivery_delta -ne 0",
            "$clientResult.deduplication_stability.instance_metrics.api_a.connection_count -ne 3",
            "$clientResult.deduplication_stability.instance_metrics.api_b.replay_query_delta -lt 1",
            "$clientResult.deduplication_stability.instance_metrics.api_b.total_replay_query_delta -lt 2",
            "$clientResult.deduplication_stability.instance_metrics.api_b.delivery_delta -ne 0",
            "$clientResult.deduplication_stability.instance_metrics.api_b.connection_count -ne 1",
            "@($clientResult.deduplication_stability.probe_counts).Count -ne 4",
            "@($clientResult.deduplication_stability.final_probe_counts).Count -ne 4",
            "$clientResult.offline_reconnect.ack_persistence.full_replay_cycles -ne 2",
            "$clientResult.offline_reconnect.ack_persistence.post_ack_message_count -ne 0",
            "$clientResult.offline_reconnect.ack_persistence.delivery_delta -ne 0",
            "$clientResult.redis_recovery_stability.raw_frame_counts_unchanged -ne $true",
            "$clientResult.redis_recovery_stability.api_a_delivery_delta -ne 0",
            "$clientResult.redis_recovery_stability.api_b_delivery_delta -ne 0",
            "@($clientResult.redis_recovery_stability.probe_counts).Count -ne 4",
        ):
            self.assertIn(fragment, self.stage + self.client)

        offline_final = self.client.index(
            "const deliveryEvidence = await settleSingleProbeDelivery(",
            self.client.index("async function assertOfflineReconnect("),
        )
        offline_close = self.client.index("await closeProbes([reconnectProbe])", offline_final)
        ack_persistence = self.client.index(
            "const ackPersistence = await assertAcknowledgedMessageAbsentAcrossReplayCycles(",
            offline_close,
        )
        offline_read = self.client.index(
            "await requestJson(secondaryApiBase, `/api/v1/system/messages/${messageId}/read`",
            ack_persistence,
        )
        self.assertLess(offline_final, offline_close)
        self.assertLess(offline_close, ack_persistence)
        self.assertLess(ack_persistence, offline_read)

    def test_redis_recovery_preserves_acknowledged_raw_frame_snapshot(self) -> None:
        for fragment in (
            "function assertProbeCountsUnchanged(probes, expectedCounts, label)",
            "current.raw_frame_count !== expectedCounts[index].raw_frame_count",
            "const deliveryProbeCounts = deliveryEvidence.probeCounts",
            '"Redis 恢复后检查"',
            "recoveryApiA.delivered !== finalMetrics.api_a.delivered",
            "recoveryApiB.delivered !== finalMetrics.api_b.delivered",
            "raw_frame_counts_unchanged: true",
            "redis_recovery_stability: redisRecoveryStability",
        ):
            self.assertIn(fragment, self.client)
        delivered = self.client.index("writeJsonAtomically(deliveredPath")
        restored = self.client.index('"Redis 恢复信号"', delivered)
        snapshot_check = self.client.index('"Redis 恢复后检查"', restored)
        deduplication = self.client.index(
            "const deduplicationStability = await assertReplayDeduplicationWindow(",
            snapshot_check,
        )
        self.assertLess(delivered, restored)
        self.assertLess(restored, snapshot_check)
        self.assertLess(snapshot_check, deduplication)

    def test_removed_message_contract_fields_cannot_return(self) -> None:
        contract = self.stage + self.client
        for pattern in (
            r"\btargetCount\b",
            r"\btarget_count\b",
            r"\.acknowledgement\b",
            r"offline_reconnect\.(?:message_count|stability_window)\b",
            r"\bisolated_connection_count\b",
            r"\bpre_ack_probe_counts\b",
        ):
            self.assertNotRegex(contract, pattern)

    def test_no_placeholder_can_be_reported_as_passed(self) -> None:
        client_result = self.client.index("await writeJsonAtomically(resultPath, {")
        delivered = self.client.index("writeJsonAtomically(deliveredPath")
        restored = self.client.index('"Redis 恢复信号"')
        persisted = self.client.index("assertAckAndReadPersistence", restored)
        stable = self.client.index(
            "const deduplicationStability = await assertReplayDeduplicationWindow(",
            persisted,
        )
        cleanup_ready = self.client.index("writeJsonAtomically(cleanupReadyPath", stable)
        cleanup = self.client.index("assertRetentionCleanup", cleanup_ready)
        final_probe_check = self.client.index(
            '"关闭前统一检查"',
            cleanup,
        )
        closed = self.client.index("await closeProbes(allProbes)", final_probe_check)
        self.assertLess(delivered, restored)
        self.assertLess(restored, persisted)
        self.assertLess(persisted, stable)
        self.assertLess(stable, cleanup_ready)
        self.assertLess(cleanup_ready, cleanup)
        self.assertLess(cleanup, final_probe_check)
        self.assertLess(final_probe_check, closed)
        self.assertLess(closed, client_result)

        result_check = self.stage.index('$clientResult.status -ne "passed"')
        run_succeeded = self.stage.index("$runSucceeded = $true", result_check)
        terminal_passed = self.stage.index('$metadata["status"] = "passed"', run_succeeded)
        self.assertLess(result_check, run_succeeded)
        self.assertLess(run_succeeded, terminal_passed)
        self.assertNotIn("AllowFailure", self.stage + self.client)

    def test_ticket_expiry_replay_and_origin_are_real_handshakes(self) -> None:
        for fragment in (
            'Set-MessageAcceptanceEnvironment -Name "APP_MESSAGING_TICKET_TTL_SECONDS" -Value "2"',
            'import { connect } from "node:net"',
            "async function rawUpgradeStatus",
            'headers.push(`Origin: ${origin}`)',
            '"https://untrusted.invalid"',
            "if (expiredStatus !== 401)",
            "if (wrongOriginStatus !== 403)",
            "const hello = await consumeTicketAndClose",
            "const replayStatus = await rawUpgradeStatus(apiBase, origin.ticket)",
            "if (replayStatus !== 401)",
            "$clientReady.ticket_guards.expired_status -ne 401",
            "$clientReady.ticket_guards.wrong_origin_status -ne 403",
            "$clientReady.ticket_guards.replay_status -ne 401",
        ):
            self.assertIn(fragment, self.stage + self.client)
        rejected = self.client.index("const wrongOriginStatus = await rawUpgradeStatus")
        accepted = self.client.index("const hello = await consumeTicketAndClose", rejected)
        replayed = self.client.index("const replayStatus = await rawUpgradeStatus", accepted)
        self.assertLess(rejected, accepted)
        self.assertLess(accepted, replayed)

    def test_slow_consumer_requires_exact_1013_and_persistent_cleanup(self) -> None:
        for fragment in (
            'Set-MessageAcceptanceEnvironment -Name "APP_MESSAGING_OUTBOUND_BUFFER" -Value "4"',
            'Set-MessageAcceptanceEnvironment -Name "TOKIO_WORKER_THREADS" -Value "1"',
            "for (let index = 0; index < 16; index += 1)",
            '"载荷".repeat(4_000)',
            "if (event.code !== 1013)",
            "response?.data?.inserted !== true",
            'waitFor("慢消费者积压消息完整持久化", 5_000',
            "matched.length === backlogIds.length ? matched : null",
            '"/api/v1/system/messages/read-all"',
            'waitFor("慢消费者积压消息全部回读为已读", 5_000',
            "$clientReady.slow_consumer.close_code -ne 1013",
            "$clientReady.slow_consumer.backlog_count -ne 16",
            "$clientReady.slow_consumer.persisted_count -ne 16",
            "$clientReady.slow_consumer.read_back_count -ne 16",
        ):
            self.assertIn(fragment, self.stage + self.client)

    def test_ack_and_read_are_verified_across_api_instances(self) -> None:
        for fragment in (
            '"--secondary-api-base"',
            "acknowledgedIds: new Set()",
            "acknowledgementRequestedIds: new Set()",
            'socket.send(JSON.stringify({ v: 1, type: "ack", ids: [state.target.id] }))',
            "for (const id of frame.ids) state.acknowledgedIds.add(String(id))",
            "assertAcknowledgedMessageAbsentAcrossReplayCycles(",
            'record?.acked_at && record?.read_at === null',
            '`/api/v1/system/messages/${messageId}/read`',
            'record?.acked_at && record?.read_at ? record : null',
            "verified_across_instances: true",
            "$clientResult.persisted_state.verified_across_instances -ne $true",
            "$clientResult.persisted_state.acked_at",
            "$clientResult.persisted_state.read_at",
        ):
            self.assertIn(fragment, self.stage + self.client)
        ack = self.client.index('type: "ack", ids: [state.target.id]')
        persisted = self.client.index("assertAckAndReadPersistence", ack)
        self.assertLess(ack, persisted)
        ack_handler = self.client.index("if (frame.type === \"ack\" && Array.isArray(frame.ids))")
        self.assertNotIn("&& state.target", self.client[ack_handler:ack_handler + 180])

    def test_tenant_isolation_uses_isolated_database_and_live_delivery(self) -> None:
        for fragment in (
            '"--database=ryframe_test"',
            '"compose", "--project-name", $ProjectName,',
            '"--file", $ComposeFile,',
            '"--file", $OwnershipComposeFile,',
            "INSERT INTO sys_tenant",
            "INSERT INTO sys_user",
            "INSERT INTO sys_config",
            "'sys.account.captchaEnabled', 'false'",
            "'runtime acceptance only', '0'",
            "AND del_flag = '0'",
            "runtime-isolated-tenant",
            "INSERT INTO sys_message (",
            "INSERT INTO sys_message_recipient",
            '"ryframe:message:dispatch", $MessageId',
            '-Expected "2"',
            'tenant_id = "runtime-isolated"',
            "const isolated = await login(secondaryApiBase, tenantId, username)",
            '"隔离租户连接"',
            "system_inbox_count: systemInboxCount",
            "system_connection_count: systemConnectionCount",
            "isolated_inbox_count: isolatedInboxCount",
            "isolated_logical_message_count: isolatedProbe.state.targetMessageIds.size",
            "isolated_raw_frame_count: isolatedProbe.state.targetRawFrameCount",
            '$tenantResult.system_connection_count -ne 0',
            '$tenantResult.isolated_logical_message_count -ne 1',
            '$tenantResult.isolated_raw_frame_count -lt 1',
            '-Expected "1:1:1:1:1"',
        ):
            self.assertIn(fragment, self.stage + self.client)
        self.assertNotIn("DROP DATABASE", self.stage.upper())
        self.assertNotIn("CREATE DATABASE", self.stage.upper())
        self.assertNotIn("消息验收隔离租户", self.stage)
        self.assertNotIn("消息验收隔离用户", self.stage)
        publish = self.stage.index("Invoke-MessageAcceptanceRedisPublish")
        fixture_signal = self.stage.index("-Path $tenantFixturePath", publish)
        result_wait = self.stage.index("-Path $tenantResultPath", fixture_signal)
        self.assertLess(publish, fixture_signal)
        self.assertLess(fixture_signal, result_wait)

    def test_retention_proves_policy_limit_and_real_worker_cascade_deletion(self) -> None:
        for fragment in (
            'Set-MessageAcceptanceEnvironment -Name "APP_MESSAGING_RETENTION_DAYS" -Value "90"',
            "const expectedRetentionSeconds = 90 * 24 * 60 * 60",
            "Math.abs(retentionSeconds - expectedRetentionSeconds) > 5",
            "Date.now() + 91 * 24 * 60 * 60 * 1_000",
            'overLimitResponse.status !== 400 || overLimitBody?.error_key !== "validation"',
            "$defaultRetentionSeconds -lt 7775995",
            "$defaultRetentionSeconds -gt 7776005",
            '$cleanupReady.over_limit_error_key -ne "validation"',
            "source_type = 'runtime_acceptance_0_7_retention'",
            "UTC_TIMESTAMP() - INTERVAL 91 DAY",
            "expires_at = UTC_TIMESTAMP() - INTERVAL 1 DAY",
            "WHERE id = $retentionMessageIdValue",
            "SET @retention_job_count := (",
            "SET @retention_job_id := (",
            "SELECT MIN(id)",
            "SET priority = 2147483647, available_at = UTC_TIMESTAMP()",
            "WHERE id = @retention_job_id",
            "AND @retention_job_count = 1",
            "SET @retention_job_updated := ROW_COUNT()",
            "$retentionPrepareFields.Count -ne 6",
            '$retentionPrepareFields[5] -ne "1"',
            '-Executable $workerBinary',
            '-Arguments @("--once")',
            "SELECT COUNT(*) FROM sys_message_audience WHERE message_id = $retentionMessageIdValue",
            "SELECT COUNT(*) FROM sys_message_recipient WHERE message_id = $retentionMessageIdValue",
            '-Expected "0:0:0:succeeded:1:1"',
            "evidence?.retention_days !== 90",
            "evidence?.over_limit_status !== 400",
            'evidence?.over_limit_error_key !== "validation"',
            "evidence?.aged_days < 90",
            'evidence?.job_status !== "succeeded"',
            '$clientResult.retention_cleanup.retention_days -ne 90',
            '$clientResult.retention_cleanup.over_limit_status -ne 400',
            '$clientResult.retention_cleanup.over_limit_error_key -ne "validation"',
            '$clientResult.retention_cleanup.job_status -ne "succeeded"',
        ):
            self.assertIn(fragment, self.stage + self.client)
        self.assertNotIn(
            "UPDATE sys_background_job\nSET priority = 2147483647, available_at = UTC_TIMESTAMP()\nWHERE job_type",
            self.stage,
        )
        self.assertNotIn("LIMIT 1", self.stage)
        ready = self.stage.index("-Path $cleanupReadyPath")
        prepare = self.stage.index("$retentionPrepareSql", ready)
        worker = self.stage.index('-Executable $workerBinary', prepare)
        verify = self.stage.index("$retentionVerifySql", worker)
        signal = self.stage.index("-Path $cleanupResultPath", verify)
        self.assertLess(ready, prepare)
        self.assertLess(prepare, worker)
        self.assertLess(worker, verify)
        self.assertLess(verify, signal)

    def test_failure_cleanup_covers_processes_redis_and_project(self) -> None:
        finally_position = self.stage.rindex("finally {")
        terminal = self.stage[finally_position:]
        for fragment in (
            "Restore-RyFrameV07DockerFault",
            "Stop-MessageAcceptanceProcess",
            "Remove-RyFrameV07DockerProjectResources",
            "Stop-Transcript",
            '$metadata["cleanup_errors"] = @($cleanupErrors)',
            '$metadata["status"] = "cleanup_failed"',
            "Write-RyFrameV07MetadataAtomically",
        ):
            self.assertIn(fragment, terminal)
        self.assertLess(
            terminal.index("Remove-RyFrameV07DockerProjectResources"),
            terminal.index("Write-RyFrameV07MetadataAtomically"),
        )

    def test_messages_and_comments_follow_the_chinese_policy(self) -> None:
        match = re.search(
            r"\$script:MessageAcceptanceMessages = ConvertFrom-Json @'\n(.*?)\n'@",
            self.stage,
            re.DOTALL,
        )
        self.assertIsNotNone(match)
        messages = json.loads(match.group(1))
        for message in messages.values():
            self.assertRegex(message, r"[\u4e00-\u9fff]")
        self.assertNotRegex(self.stage, r'-Description\s+"[A-Za-z]')
        self.assertNotRegex(
            self.client,
            re.compile(r"^\s*//\s*[A-Za-z]", re.MULTILINE),
        )

    def test_static_syntax_checks_pass_when_tools_are_available(self) -> None:
        executable_name = "powershell.exe" if os.name == "nt" else "pwsh"
        powershell = shutil.which(executable_name)
        if powershell is not None:
            escaped = str(STAGE).replace("'", "''")
            command = (
                "$tokens=$null;$errors=$null;"
                "[void][System.Management.Automation.Language.Parser]::ParseFile("
                f"'{escaped}',[ref]$tokens,[ref]$errors);"
                "if($errors.Count -gt 0){$errors|ForEach-Object{Write-Error $_.Message};exit 1}"
            )
            arguments = [powershell, "-NoLogo", "-NoProfile", "-NonInteractive"]
            if os.name == "nt":
                arguments.extend(["-ExecutionPolicy", "Bypass"])
            arguments.extend(["-Command", command])
            result = subprocess.run(
                arguments,
                cwd=ROOT,
                check=False,
                capture_output=True,
                timeout=30,
            )
            output = (result.stdout + result.stderr).decode(errors="replace")
            self.assertEqual(result.returncode, 0, output)

        node = shutil.which("node")
        if node is not None:
            result = subprocess.run(
                [node, "--check", str(CLIENT)],
                cwd=ROOT,
                check=False,
                capture_output=True,
                timeout=30,
            )
            output = (result.stdout + result.stderr).decode(errors="replace")
            self.assertEqual(result.returncode, 0, output)


if __name__ == "__main__":
    unittest.main()
