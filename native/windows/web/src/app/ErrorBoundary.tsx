import { Component, type ErrorInfo, type ReactNode } from 'react';
import { ServicesContext, type AppServices } from './context';

export type ErrorBoundaryScope = 'app' | 'pane' | 'dialog';

export interface ErrorBoundaryProps {
  scope: ErrorBoundaryScope;
  /** Changing this key automatically clears a caught error (e.g. switching panes or dialogs). */
  resetKey?: string | number;
  children?: ReactNode;
}

interface ErrorBoundaryState {
  error: Error | null;
  resetKey: string | number | undefined;
}

const scopeTitle: Record<ErrorBoundaryScope, string> = {
  app: 'Something went wrong.',
  pane: 'This pane encountered an error.',
  dialog: 'This dialog encountered an error.',
};

/**
 * Isolates rendering errors to an app, pane or dialog boundary, reports them to the host log bridge,
 * and offers a recoverable Retry action (or automatic reset when `resetKey` changes).
 */
export class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  static override contextType = ServicesContext;
  declare context: AppServices | null;

  constructor(props: ErrorBoundaryProps) {
    super(props);
    this.state = { error: null, resetKey: props.resetKey };
  }

  static getDerivedStateFromError(error: Error): Partial<ErrorBoundaryState> {
    return { error };
  }

  static getDerivedStateFromProps(
    props: ErrorBoundaryProps,
    state: ErrorBoundaryState,
  ): Partial<ErrorBoundaryState> | null {
    if (props.resetKey !== state.resetKey) {
      return { error: null, resetKey: props.resetKey };
    }
    return null;
  }

  override componentDidCatch(error: Error, info: ErrorInfo): void {
    const stack = info.componentStack?.trim();
    const detail = error.message || String(error);
    const message = stack
      ? `[ErrorBoundary:${this.props.scope}] ${detail}\n${stack}`
      : `[ErrorBoundary:${this.props.scope}] ${detail}`;
    void this.context?.bridge.call('log', { level: 'error', message }).catch(() => {});
  }

  private readonly retry = (): void => {
    this.setState({ error: null });
  };

  override render(): ReactNode {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <div
        className={`error-boundary error-boundary-${this.props.scope}`}
        role="alert"
        data-testid={`error-boundary-${this.props.scope}`}
      >
        <div className="error-boundary-card">
          <div className="error-boundary-title">{scopeTitle[this.props.scope]}</div>
          {error.message && <div className="error-boundary-detail">{error.message}</div>}
          <button
            type="button"
            className="btn error-boundary-retry"
            data-testid="error-boundary-retry"
            onClick={this.retry}
          >
            Retry
          </button>
        </div>
      </div>
    );
  }
}
