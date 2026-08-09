import type { ButtonHTMLAttributes } from 'react';
import { cn } from '../../lib/utils';

type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: 'primary' | 'ghost' | 'outline' | 'icon';
  size?: 'default' | 'sm';
};

export function Button({ className, variant = 'ghost', size = 'default', type = 'button', ...props }: ButtonProps) {
  return <button type={type} className={cn('btn', `btn--${variant}`, size === 'sm' && 'btn--sm', className)} {...props} />;
}
