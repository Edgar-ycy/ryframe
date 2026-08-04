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
            "redis-down.signal",
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
            'Set-MessageAcceptanceEnvironment -Name "APP_MESSAGING_ENABLED" -Value "true"',
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
        down_signal = self.stage.index("Write-MessageAcceptanceSignal -Path $redisDownSignal")
        delivered = self.stage.index("-Path $clientDeliveredPath", down_signal)
        restore = self.stage.index("Restore-RyFrameV07DockerFault", delivered)
        restored_signal = self.stage.index(
            "Write-MessageAcceptanceSignal -Path $redisRestoredSignal", restore
        )
        self.assertLess(stop, down_signal)
        self.assertLess(down_signal, delivered)
        self.assertLess(delivered, restore)
        self.assertLess(restore, restored_signal)
        for fragment in (
            "$apiAInterruptedBefore + 1",
            "$apiBInterruptedBefore + 1",
            "$apiASubscribedBefore + 1",
            "$apiBSubscribedBefore + 1",
            '$metadata["redis_fault"]["interrupted_instance_count"] = 2',
            '$metadata["redis_fault"]["restored_instance_count"] = 2',
        ):
            self.assertIn(fragment, self.stage)
        self.assertNotIn("toxiproxy", (self.stage + self.client).lower())
        self.assertNotRegex(self.stage, r'@\("(?:container|network)",\s*"(?:stop|start|disconnect|connect)"')

    def test_client_proves_multi_connection_bilingual_exact_delivery(self) -> None:
        for fragment in (
            '{ locale: "zh-CN", text: expectedZh, label: "中文连接一" }',
            '{ locale: "en-US", text: expectedEn, label: "英文连接" }',
            '{ locale: "zh-CN", text: expectedZh, label: "中文连接二" }',
            'label: "API-B 英文连接"',
            'headers: authHeaders(token, locale)',
            'title_key: "user.welcome"',
            'body_key: "user.welcome"',
            'audiences: [{ kind: "user", target_id: userId }]',
            "state.targetCount > 1",
            "allProbes.every((probe) => probe.state.targetCount === 1)",
            'assertInboxRendering(apiBase, token, messageId, "zh-CN", expectedZh)',
            'assertInboxRendering(apiBase, token, messageId, "en-US", expectedEn)',
        ):
            self.assertIn(fragment, self.client)
        down_wait = self.client.index('waitFor("Redis 中断信号"')
        publish = self.client.index("publishLocalizedMessage", down_wait)
        delivered_write = self.client.index("writeJsonAtomically(deliveredPath", publish)
        restore_wait = self.client.index('waitFor("Redis 恢复信号"', delivered_write)
        self.assertLess(down_wait, publish)
        self.assertLess(publish, delivered_write)
        self.assertLess(delivered_write, restore_wait)

    def test_same_identity_is_active_on_both_instances_with_exact_metrics(self) -> None:
        for fragment in (
            "const secondaryGrant = await issueTicket(",
            "secondaryApiBase,",
            "secondaryProbe = createSocketProbe(",
            "const allProbes = [...probes, secondaryProbe]",
            "await waitForHealthyProbes(allProbes)",
            "alignReplayBaseline(apiBase, probes.length)",
            "alignReplayBaseline(secondaryApiBase, 1)",
            "primaryReplayQueryDelta !== 1 || secondaryReplayQueryDelta !== 1",
            "primaryDeliveryDelta !== probes.length || secondaryDeliveryDelta !== 1",
            "primary_connection_count: probes.length",
            "secondary_connection_count: 1",
            "total_connection_count: allProbes.length",
            "api_a: {",
            "api_b: {",
            "$clientDelivered.primary_connection_count -ne 3",
            "$clientDelivered.secondary_connection_count -ne 1",
            "$clientDelivered.total_connection_count -ne 4",
            "$clientDelivered.instance_metrics.api_a.replay_query_delta -ne 1",
            "$clientDelivered.instance_metrics.api_a.delivery_delta -ne 3",
            "$clientDelivered.instance_metrics.api_b.replay_query_delta -ne 1",
            "$clientDelivered.instance_metrics.api_b.delivery_delta -ne 1",
        ):
            self.assertIn(fragment, self.stage + self.client)

        primary_grants = self.client.index("const grants = await Promise.all(")
        secondary_grant = self.client.index("const secondaryGrant = await issueTicket(")
        combined = self.client.index("const allProbes = [...probes, secondaryProbe]")
        target_wait = self.client.index(
            "allProbes.every((probe) => probe.state.targetCount === 1)"
        )
        self.assertLess(primary_grants, secondary_grant)
        self.assertLess(secondary_grant, combined)
        self.assertLess(combined, target_wait)

    def test_offline_window_reconnects_to_secondary_once(self) -> None:
        for fragment in (
            "async function assertOfflineReconnect(",
            "settledMetrics(primaryApiBase, 0)",
            "settledMetrics(secondaryApiBase, 0)",
            '"runtime_acceptance_0_7_offline"',
            "whileOffline[index].connections !== 0",
            "whileOffline[index].delivered !== offlineBaselines[index].delivered",
            "const reconnectGrant = await issueTicket(secondaryApiBase",
            "createSocketProbe(\n    secondaryApiBase,",
            "reconnectProbe.state.targetCount === 1",
            "replayDelta !== 1 || deliveryDelta !== 1",
            "primaryAfterReconnect.replaySuccess !== offlineBaselines[0].replaySuccess",
            "await closeProbes([reconnectProbe])",
            "const offlineReconnect = await assertOfflineReconnect(",
            "offline_reconnect: offlineReconnect",
            '$clientReady.offline_reconnect.disconnected_instance -ne "api_a"',
            '$clientReady.offline_reconnect.reconnected_instance -ne "api_b"',
            "$clientReady.offline_reconnect.message_count -ne 1",
            "$clientDelivered.offline_reconnect.message_count -ne 1",
            "$clientResult.offline_reconnect.message_count -ne 1",
        ):
            self.assertIn(fragment, self.stage + self.client)

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
            "alignReplayBaseline(apiBase, probes.length)",
            "alignReplayBaseline(secondaryApiBase, 1)",
            "primaryReplayQueryDelta !== 1 || secondaryReplayQueryDelta !== 1",
            "primaryDeliveryDelta !== probes.length || secondaryDeliveryDelta !== 1",
            "replay_query_delta: primaryReplayQueryDelta",
            "delivery_delta: primaryDeliveryDelta",
            "replay_query_delta: secondaryReplayQueryDelta",
            "delivery_delta: secondaryDeliveryDelta",
        ):
            self.assertIn(fragment, self.client)
        for fragment in (
            "$clientDelivered.instance_metrics.api_a.replay_query_delta -ne 1",
            "$clientDelivered.instance_metrics.api_a.delivery_delta -ne 3",
            "$clientDelivered.instance_metrics.api_b.replay_query_delta -ne 1",
            "$clientDelivered.instance_metrics.api_b.delivery_delta -ne 1",
            "$clientResult.instance_metrics.api_a.replay_query_delta -ne 1",
            "$clientResult.instance_metrics.api_b.replay_query_delta -ne 1",
        ):
            self.assertIn(fragment, self.stage)

    def test_deduplication_stability_crosses_a_full_later_replay_cycle(self) -> None:
        for fragment in (
            "function assertTargetProbesStable(probes, label)",
            "async function waitForReplayAdvance(instance, baseline, probes, label)",
            "async function assertReplayDeduplicationWindow(instances, probes, label)",
            "current.replaySuccess > baseline.replaySuccess",
            "current.delivered !== baseline.delivered",
            "waitForReplayAdvance(instance, starting[index]",
            "waitForReplayAdvance(instance, aligned[index]",
            "completed[index].replaySuccess <= aligned[index].replaySuccess",
            "full_replay_cycle_observed: true",
            "total_replay_query_delta: final.replaySuccess - starting[index].replaySuccess",
            "delivery_delta: final.delivered - starting[index].delivered",
            "const stabilityWindow = await assertReplayDeduplicationWindow(",
            '[{ name: "api_b", apiBase: secondaryApiBase, connectionCount: 1 }]',
            "const deduplicationStability = await assertReplayDeduplicationWindow(",
            '{ name: "api_a", apiBase, connectionCount: probes.length }',
            '{ name: "api_b", apiBase: secondaryApiBase, connectionCount: 1 }',
            'assertTargetProbesStable([reconnectProbe], "断线重连关闭前检查")',
            'assertTargetProbesStable(allProbes, "关闭前统一检查")',
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
            "$clientResult.offline_reconnect.stability_window.full_replay_cycle_observed -ne $true",
            "$clientResult.offline_reconnect.stability_window.instance_metrics.api_b.total_replay_query_delta -lt 2",
            "$clientResult.offline_reconnect.stability_window.instance_metrics.api_b.delivery_delta -ne 0",
        ):
            self.assertIn(fragment, self.stage + self.client)

        offline_read = self.client.index(
            "await requestJson(secondaryApiBase, `/api/v1/system/messages/${messageId}/read`",
            self.client.index("async function assertOfflineReconnect("),
        )
        offline_stability = self.client.index(
            "const stabilityWindow = await assertReplayDeduplicationWindow(",
            offline_read,
        )
        offline_final = self.client.index(
            'assertTargetProbesStable([reconnectProbe], "断线重连关闭前检查")',
            offline_stability,
        )
        offline_close = self.client.index("await closeProbes([reconnectProbe])", offline_final)
        self.assertLess(offline_read, offline_stability)
        self.assertLess(offline_stability, offline_final)
        self.assertLess(offline_final, offline_close)

    def test_no_placeholder_can_be_reported_as_passed(self) -> None:
        client_result = self.client.index("await writeJsonAtomically(resultPath, {")
        delivered = self.client.index("writeJsonAtomically(deliveredPath")
        restored = self.client.index('waitFor("Redis 恢复信号"')
        persisted = self.client.index("assertAckAndReadPersistence", restored)
        stable = self.client.index(
            "const deduplicationStability = await assertReplayDeduplicationWindow(",
            persisted,
        )
        cleanup_ready = self.client.index("writeJsonAtomically(cleanupReadyPath", stable)
        cleanup = self.client.index("assertRetentionCleanup", cleanup_ready)
        final_probe_check = self.client.index(
            'assertTargetProbesStable(allProbes, "关闭前统一检查")',
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
            "for (let index = 0; index < 16; index += 1)",
            '"载荷".repeat(4_000)',
            "if (event.code !== 1013)",
            '"/api/v1/system/messages/read-all"',
            "marked.data < backlogIds.length",
            "$clientReady.slow_consumer.close_code -ne 1013",
            "$clientReady.slow_consumer.backlog_count -lt 16",
            "$clientReady.slow_consumer.marked_read_count -lt 16",
        ):
            self.assertIn(fragment, self.stage + self.client)

    def test_ack_and_read_are_verified_across_api_instances(self) -> None:
        for fragment in (
            '"--secondary-api-base"',
            'probes[0].socket.send(JSON.stringify({ v: 1, type: "ack", ids: [messageId] }))',
            'record?.acked_at && record?.read_at === null',
            '`/api/v1/system/messages/${messageId}/read`',
            'record?.acked_at && record?.read_at ? record : null',
            "verified_across_instances: true",
            "$clientResult.persisted_state.verified_across_instances -ne $true",
            "$clientResult.persisted_state.acked_at",
            "$clientResult.persisted_state.read_at",
        ):
            self.assertIn(fragment, self.stage + self.client)
        ack = self.client.index('type: "ack", ids: [messageId]')
        persisted = self.client.index("assertAckAndReadPersistence", ack)
        self.assertLess(ack, persisted)

    def test_tenant_isolation_uses_isolated_database_and_live_delivery(self) -> None:
        for fragment in (
            '"--database=ryframe_test"',
            '"compose", "--project-name", $ProjectName,',
            '"--file", $ComposeFile,',
            '"--file", $OwnershipComposeFile,',
            "INSERT INTO sys_tenant",
            "INSERT INTO sys_user",
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
            "isolated_connection_count: isolatedProbe.state.targetCount",
            '$tenantResult.system_connection_count -ne 0',
            '$tenantResult.isolated_connection_count -ne 1',
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

    def test_retention_runs_the_real_worker_and_proves_cascade_deletion(self) -> None:
        for fragment in (
            "source_type = 'runtime_acceptance_0_7_retention'",
            "UTC_TIMESTAMP() - INTERVAL 91 DAY",
            "expires_at = UTC_TIMESTAMP() - INTERVAL 1 DAY",
            "WHERE id = $retentionMessageIdValue",
            "WHERE job_type = 'system.message.retention' AND status = 'pending'",
            "SET priority = 2147483647, available_at = UTC_TIMESTAMP()",
            '-Executable $workerBinary',
            '-Arguments @("--once")',
            "SELECT COUNT(*) FROM sys_message_audience WHERE message_id = $retentionMessageIdValue",
            "SELECT COUNT(*) FROM sys_message_recipient WHERE message_id = $retentionMessageIdValue",
            '-Expected "0:0:0:succeeded:1:1"',
            "evidence?.aged_days < 90",
            'evidence?.job_status !== "succeeded"',
            '$clientResult.retention_cleanup.job_status -ne "succeeded"',
        ):
            self.assertIn(fragment, self.stage + self.client)
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
