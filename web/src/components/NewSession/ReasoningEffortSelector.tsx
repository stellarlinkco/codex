import type { ReasoningEffort } from '@/types/api'
import { useTranslation } from '@/lib/use-translation'

export type ReasoningEffortPreference = ReasoningEffort | 'auto'

const OPTIONS: Array<{ value: ReasoningEffortPreference; labelKey: string }> = [
    { value: 'auto', labelKey: 'newSession.reasoningEffort.auto' },
    { value: 'none', labelKey: 'newSession.reasoningEffort.none' },
    { value: 'minimal', labelKey: 'newSession.reasoningEffort.minimal' },
    { value: 'low', labelKey: 'newSession.reasoningEffort.low' },
    { value: 'medium', labelKey: 'newSession.reasoningEffort.medium' },
    { value: 'high', labelKey: 'newSession.reasoningEffort.high' },
    { value: 'xhigh', labelKey: 'newSession.reasoningEffort.xhigh' },
]

export function ReasoningEffortSelector(props: {
    reasoningEffort: ReasoningEffortPreference
    isDisabled: boolean
    onReasoningEffortChange: (value: ReasoningEffortPreference) => void
}) {
    const { t } = useTranslation()

    return (
        <div className="flex flex-col gap-1.5 px-3 py-3">
            <label className="text-xs font-medium text-[var(--app-hint)]">
                {t('newSession.reasoningEffort')}{' '}
                <span className="font-normal">({t('newSession.reasoningEffort.optional')})</span>
            </label>
            <select
                value={props.reasoningEffort}
                onChange={(e) => props.onReasoningEffortChange(e.target.value as ReasoningEffortPreference)}
                disabled={props.isDisabled}
                className="w-full px-3 py-2 text-sm rounded-lg border border-[var(--app-divider)] bg-[var(--app-bg)] text-[var(--app-text)] focus:outline-none focus:ring-2 focus:ring-[var(--app-link)] disabled:opacity-50"
            >
                {OPTIONS.map((option) => (
                    <option key={option.value} value={option.value}>
                        {t(option.labelKey)}
                    </option>
                ))}
            </select>
        </div>
    )
}

