import { useEffect, useState } from 'react';
/** 系统的「减弱动态效果」设置。 */
export function useSystemReducedMotion() {
    const [reduced, setReduced] = useState(() => typeof window === 'undefined' ? false : window.matchMedia('(prefers-reduced-motion: reduce)').matches);
    useEffect(() => {
        const mql = window.matchMedia('(prefers-reduced-motion: reduce)');
        const onChange = () => setReduced(mql.matches);
        mql.addEventListener('change', onChange);
        return () => mql.removeEventListener('change', onChange);
    }, []);
    return reduced;
}
