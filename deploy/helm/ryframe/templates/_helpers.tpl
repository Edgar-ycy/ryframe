{{/*
Expand the name of the chart.
*/}}
{{- define "ryframe.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Create a default fully qualified app name.
*/}}
{{- define "ryframe.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- $name := default .Chart.Name .Values.nameOverride }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}
{{- end }}

{{/*
Create chart name and version as used by the chart label.
*/}}
{{- define "ryframe.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Common labels
*/}}
{{- define "ryframe.labels" -}}
helm.sh/chart: {{ include "ryframe.chart" . }}
{{ include "ryframe.selectorLabels" . }}
{{- if .Chart.AppVersion }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
{{- end }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{/*
Selector labels
*/}}
{{- define "ryframe.selectorLabels" -}}
app.kubernetes.io/name: {{ include "ryframe.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/*
Create the name of the service account to use
*/}}
{{- define "ryframe.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}
{{- default (include "ryframe.fullname" .) .Values.serviceAccount.name }}
{{- else }}
{{- default "default" .Values.serviceAccount.name }}
{{- end }}
{{- end }}

{{/*
MySQL host string
*/}}
{{- define "ryframe.mysqlHost" -}}
{{- if .Values.mysql.enabled }}
{{- printf "%s-mysql" .Release.Name }}
{{- else }}
{{- .Values.mysql.external.host }}
{{- end }}
{{- end }}

{{/*
Redis host string
*/}}
{{- define "ryframe.redisHost" -}}
{{- if .Values.redis.enabled }}
{{- printf "%s-redis-master" .Release.Name }}
{{- else }}
{{- .Values.redis.external.host }}
{{- end }}
{{- end }}

{{/*
MySQL connection string (used in init container)
*/}}
{{- define "ryframe.mysqlService" -}}
{{ include "ryframe.mysqlHost" . }}:{{ .Values.mysql.external.port }}
{{- end }}
