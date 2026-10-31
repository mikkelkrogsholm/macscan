export const APFS='apfs-data-clone';
export const metrics=['allocated','logical',APFS];
export const metricLabel=metric=>({allocated:'Hardlink-tilskrevet allokering',logical:'Logisk størrelse',[APFS]:'Begrænset DATA-klonkorrigeret allokering'})[metric]??metric;
export const apfsSupported=(value,context=value?.context??value?.measurement_context)=>{
 const a=value?.apfs_data_clone;
 return Boolean(a&&a.algorithm_version==='apfs-data-clone-1'&&a.capture_version===1&&a.columns_available!==false&&context?.clone_capture?.requested===true&&context.clone_capture.version===1);
};
export const apfsComparable=value=>apfsSupported(value)&&value.status==='complete'&&value.apfs_data_clone.comparison_ready===true;
export const apfsUnavailable=error=>String(error).includes('apfs-data-clone unavailable');
export const unavailableNote='APFS DATA er ikke tilgængelig: målingen mangler understøttet APFS-klonmetadata. Allokeret vises. Genmål med APFS-klonmetadata slået til for en ny måling.';
export const apfsNote=value=>value?.status==='fallback'
 ?'APFS DATA · fallback: primær hardlink-tilskrevet allokering er bevaret ved manglende eller ugyldige observationer. Kun fuld DATA-deling korrigeres; ikke unik, privat eller frigørbar plads.'
 :'APFS DATA korrigerer kun fuld DATA-deling inden for målingens rod. Delvis deling og øvrige forks bevares. Ejeren vælges globalt i målingen og kan ligge uden for dette område. Ikke unik, privat eller frigørbar plads.';

export const apfsContribution=node=>({
 full_data_nonowner:'DATA tilskrives en anden global ejerpost',
 full_data_owner:'Global ejer af fuldt delt DATA',
 hardlink_nonowner:'Hardlink tilskrives en anden post i målingen',
 nonfile_retained:'Mappens eget bidrag bevares',
 limited_retained:'Primær allokering bevares; ingen fuld DATA-korrektion',
 metadata_unavailable_fallback:'Klonmetadata mangler; primær allokering bevares',
 filesystem_or_volume_fallback:'Filsystem eller volumen kunne ikke bekræftes; primær allokering bevares',
 invalid_metadata_fallback:'Ugyldig klonmetadata; primær allokering bevares',
 inconsistent_group_fallback:'Modstridende klonobservationer; primær allokering bevares',
 hardlink_observation_fallback:'Modstridende hardlinkobservationer; primær allokering bevares'
})[node.apfs_data_clone_reason]??'Begrænset DATA-tilskrivning';
export const apfsZeroNonowner=node=>node.kind==='file'&&node.own_apfs_data_clone_bytes==='0'&&['full_data_nonowner','hardlink_nonowner'].includes(node.apfs_data_clone_reason);
