{{- define "homie.name" -}}
{{- .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "homie.labels" -}}
app.kubernetes.io/name: homie
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version }}
{{- end -}}

{{- define "homie.selector" -}}
app.kubernetes.io/name: homie
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{- define "homie.serviceAccount" -}}
{{- if .Values.serviceAccount.create -}}{{ include "homie.name" . }}{{- else -}}{{ .Values.serviceAccount.name | default "default" }}{{- end -}}
{{- end -}}

{{- define "homie.image" -}}
{{- $tag := .Values.image.tag | default .Chart.AppVersion -}}
{{- if .Values.image.digest -}}
{{ .Values.image.repository }}:{{ $tag }}@{{ .Values.image.digest }}
{{- else -}}
{{ .Values.image.repository }}:{{ $tag }}
{{- end -}}
{{- end -}}
